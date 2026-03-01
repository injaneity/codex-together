use super::*;

// Return value of `Session::reconstruct_history_from_rollout`, bundling the rebuilt history with
// the resume/fork hydration metadata derived from the same replay.
#[derive(Debug)]
pub(super) struct RolloutReconstruction {
    pub(super) history: Vec<ResponseItem>,
    pub(super) previous_model: Option<String>,
    pub(super) reference_context_item: Option<TurnContextItem>,
}

// In-memory implementation of the reverse rollout source used by the current eager caller.
// When reconstruction switches to lazy on-disk loading, the equivalent source should keep the
// same "load older items on demand" contract, but page older rollout items from the session file
// instead of cloning them out of an eagerly loaded `Vec<RolloutItem>`.
//
// `-1` is the newest rollout row that already existed when reconstruction state was created.
// Older persisted rows are more negative, and any rows appended after startup will be `0`, `1`,
// `2`, and so on. The future file-backed source should expose the same "read older items / replay
// forward from this location" contract, but can back that location with an opaque file cursor
// instead of an in-memory signed index.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct RolloutIndex(i64);

#[derive(Clone, Debug)]
struct InMemoryReverseRolloutSource {
    rollout_items: Vec<RolloutItem>,
    startup_rollout_len: i64,
}

impl InMemoryReverseRolloutSource {
    fn new(rollout_items: Vec<RolloutItem>) -> Self {
        let startup_rollout_len = match i64::try_from(rollout_items.len()) {
            Ok(len) => len,
            Err(_) => panic!("rollout length should fit in i64"),
        };
        Self {
            rollout_items,
            startup_rollout_len,
        }
    }

    fn start_index(&self) -> RolloutIndex {
        RolloutIndex(-self.startup_rollout_len)
    }

    fn end_index(&self) -> RolloutIndex {
        let rollout_len = match i64::try_from(self.rollout_items.len()) {
            Ok(len) => len,
            Err(_) => panic!("rollout length should fit in i64"),
        };
        RolloutIndex(rollout_len - self.startup_rollout_len)
    }

    fn iter_forward_from(
        &self,
        start: RolloutIndex,
    ) -> impl Iterator<Item = (RolloutIndex, &RolloutItem)> + '_ {
        let start = self.actual_index_from_rollout_index(start);
        self.rollout_items[start..]
            .iter()
            .enumerate()
            .map(move |(offset, item)| {
                let offset = match i64::try_from(offset) {
                    Ok(offset) => offset,
                    Err(_) => panic!("offset should fit in i64"),
                };
                (
                    RolloutIndex(start as i64 + offset - self.startup_rollout_len),
                    item,
                )
            })
    }

    fn iter_reverse_from(
        &self,
        end: RolloutIndex,
    ) -> impl Iterator<Item = (RolloutIndex, &RolloutItem)> + '_ {
        let end = self.actual_index_from_rollout_index(end);
        self.rollout_items[..end]
            .iter()
            .enumerate()
            .rev()
            .map(move |(actual_index, item)| {
                let actual_index = match i64::try_from(actual_index) {
                    Ok(actual_index) => actual_index,
                    Err(_) => panic!("actual index should fit in i64"),
                };
                (RolloutIndex(actual_index - self.startup_rollout_len), item)
            })
    }

    fn actual_index_from_rollout_index(&self, rollout_index: RolloutIndex) -> usize {
        match usize::try_from(rollout_index.0 + self.startup_rollout_len) {
            Ok(actual_index) => actual_index,
            Err(_) => panic!("rollout index should map to a loaded rollout row"),
        }
    }
}

#[derive(Clone, Debug)]
struct ReplaySlice {
    base_history: Vec<ResponseItem>,
    // Forward replay starts after the rollout item that established `base_history`.
    rollout_suffix_start: RolloutIndex,
    // If later backtracking exhausts the currently visible history, resume reverse replay before
    // this position to uncover older hidden history.
    reverse_resume_index: RolloutIndex,
    previous_model: Option<String>,
    reference_context_item: Option<TurnContextItem>,
}

#[derive(Clone, Debug)]
pub(super) struct RolloutReconstructionState {
    source: InMemoryReverseRolloutSource,
    // Materialized history that is already known to survive. After backtracking inside the
    // current visible history, we absorb that visible history into this base and make the suffix
    // empty. If later backtracking needs older hidden turns, reverse replay resumes before
    // `reverse_resume_index`.
    base_history: Vec<ResponseItem>,
    rollout_suffix_start: RolloutIndex,
    // Reverse scans that need older hidden history should continue before this position.
    reverse_resume_index: RolloutIndex,
    previous_model: Option<String>,
    reference_context_item: Option<TurnContextItem>,
}

impl RolloutReconstructionState {
    pub(super) fn new(rollout_items: Vec<RolloutItem>) -> Self {
        let source = InMemoryReverseRolloutSource::new(rollout_items);
        let replay_slice = resolve_replay_slice(&source, source.end_index(), 0);
        Self {
            source,
            base_history: replay_slice.base_history,
            rollout_suffix_start: replay_slice.rollout_suffix_start,
            reverse_resume_index: replay_slice.reverse_resume_index,
            previous_model: replay_slice.previous_model,
            reference_context_item: replay_slice.reference_context_item,
        }
    }

    pub(super) fn apply_backtracking(
        &mut self,
        turn_context: &TurnContext,
        additional_user_turns: u32,
    ) {
        // Backtracking first tries to satisfy the request inside the currently materialized
        // history. If that exhausts the visible suffix, resume reverse replay before the older
        // hidden boundary and rebuild a new base from there.
        let current_end = self.source.end_index();
        let current_reconstruction = self.reconstruct_history(turn_context, current_end);
        let visible_user_turns = current_reconstruction
            .history
            .iter()
            .filter(|item| crate::context_manager::is_user_turn_boundary(item))
            .count();
        let additional_user_turns = usize::try_from(additional_user_turns).unwrap_or(usize::MAX);

        if additional_user_turns < visible_user_turns {
            let replay_slice = resolve_replay_slice(
                &self.source,
                current_end,
                u32::try_from(additional_user_turns).unwrap_or(u32::MAX),
            );
            let mut history = ContextManager::new();
            history.replace(current_reconstruction.history);
            history
                .drop_last_n_user_turns(u32::try_from(additional_user_turns).unwrap_or(u32::MAX));

            self.base_history = history.raw_items().to_vec();
            self.rollout_suffix_start = current_end;
            // Older hidden history still begins before the same reverse boundary. If a later
            // backtrack needs to move before this newly materialized base, resume the reverse scan
            // from the older boundary we had already discovered.
            self.previous_model = replay_slice.previous_model;
            self.reference_context_item = replay_slice.reference_context_item;
            return;
        }

        let remaining_user_turns = additional_user_turns.saturating_sub(visible_user_turns);
        let replay_slice = resolve_replay_slice(
            &self.source,
            self.reverse_resume_index,
            u32::try_from(remaining_user_turns).unwrap_or(u32::MAX),
        );
        let reconstructed = reconstruct_history_until(
            turn_context,
            &self.source,
            &replay_slice,
            self.reverse_resume_index,
        );

        self.base_history = reconstructed.history;
        self.rollout_suffix_start = current_end;
        self.reverse_resume_index = replay_slice.reverse_resume_index;
        self.previous_model = replay_slice.previous_model;
        self.reference_context_item = reconstructed.reference_context_item;
    }

    fn reconstruct_history(
        &self,
        turn_context: &TurnContext,
        end_index: RolloutIndex,
    ) -> RolloutReconstruction {
        reconstruct_history_until(
            turn_context,
            &self.source,
            &ReplaySlice {
                base_history: self.base_history.clone(),
                rollout_suffix_start: self.rollout_suffix_start,
                reverse_resume_index: self.reverse_resume_index,
                previous_model: self.previous_model.clone(),
                reference_context_item: self.reference_context_item.clone(),
            },
            end_index,
        )
    }
}

#[derive(Debug, Default)]
enum TurnReferenceContextItem {
    /// No `TurnContextItem` has been seen for this replay span yet.
    ///
    /// This differs from `Cleared`: `NeverSet` means there is no evidence this turn ever
    /// established a baseline, while `Cleared` means a baseline existed and a later compaction
    /// invalidated it. Only the latter must emit an explicit clearing segment for resume/fork
    /// hydration.
    #[default]
    NeverSet,
    /// A previously established baseline was invalidated by later compaction.
    Cleared,
    /// The latest baseline established by this replay span.
    Latest(Box<TurnContextItem>),
}

#[derive(Debug, Default)]
struct ActiveReplaySegment {
    newest_rollout_index: Option<RolloutIndex>,
    oldest_rollout_index: Option<RolloutIndex>,
    turn_id: Option<String>,
    counts_as_user_turn: bool,
    previous_model: Option<String>,
    reference_context_item: TurnReferenceContextItem,
    base_replacement_history: Option<Vec<ResponseItem>>,
}

fn turn_ids_are_compatible(active_turn_id: Option<&str>, item_turn_id: Option<&str>) -> bool {
    active_turn_id
        .is_none_or(|turn_id| item_turn_id.is_none_or(|item_turn_id| item_turn_id == turn_id))
}

fn finalize_active_segment(
    active_segment: ActiveReplaySegment,
    replay_slice: &mut Option<ReplaySlice>,
    previous_model: &mut Option<String>,
    reference_context_item: &mut TurnReferenceContextItem,
    pending_rollback_turns: &mut usize,
) {
    // Thread rollback drops the newest surviving real user-message boundaries. In replay, that
    // means skipping the next finalized segments that contain a non-contextual
    // `EventMsg::UserMessage`.
    if *pending_rollback_turns > 0 {
        if active_segment.counts_as_user_turn {
            *pending_rollback_turns -= 1;
        }
        return;
    }

    let ActiveReplaySegment {
        newest_rollout_index,
        oldest_rollout_index,
        counts_as_user_turn,
        previous_model: segment_previous_model,
        reference_context_item: segment_reference_context_item,
        base_replacement_history,
        ..
    } = active_segment;

    // `previous_model` comes from the newest surviving user turn that established one.
    if previous_model.is_none() && counts_as_user_turn {
        *previous_model = segment_previous_model;
    }

    // `reference_context_item` comes from the newest surviving user turn baseline, or
    // from a surviving compaction that explicitly cleared that baseline.
    if matches!(reference_context_item, TurnReferenceContextItem::NeverSet)
        && (counts_as_user_turn
            || matches!(
                segment_reference_context_item,
                TurnReferenceContextItem::Cleared
            ))
    {
        *reference_context_item = segment_reference_context_item;
    }

    if replay_slice.is_none()
        && let Some(replacement_history) = base_replacement_history
    {
        let newest_rollout_index = match newest_rollout_index {
            Some(index) => index,
            None => panic!("active replay segment should contain rollout items"),
        };
        let oldest_rollout_index = match oldest_rollout_index {
            Some(index) => index,
            None => panic!("active replay segment should contain rollout items"),
        };
        *replay_slice = Some(ReplaySlice {
            base_history: replacement_history,
            rollout_suffix_start: RolloutIndex(newest_rollout_index.0 + 1),
            reverse_resume_index: oldest_rollout_index,
            previous_model: None,
            reference_context_item: None,
        });
    }
}

fn resolve_replay_slice(
    source: &InMemoryReverseRolloutSource,
    end_index: RolloutIndex,
    additional_user_turns: u32,
) -> ReplaySlice {
    // Shared reverse scan for both startup reconstruction and later backtracking. It finds the
    // newest surviving compaction replacement (or start of file) and the replay metadata that
    // should accompany forward materialization from that point.
    let mut replay_slice = None;
    let mut previous_model = None;
    let mut reference_context_item = TurnReferenceContextItem::NeverSet;
    let mut pending_rollback_turns = usize::try_from(additional_user_turns).unwrap_or(usize::MAX);
    let mut active_segment: Option<ActiveReplaySegment> = None;

    for (item_index, item) in source.iter_reverse_from(end_index) {
        if active_segment.is_none()
            && pending_rollback_turns == 0
            && replay_slice.is_some()
            && previous_model.is_some()
            && !matches!(reference_context_item, TurnReferenceContextItem::NeverSet)
        {
            break;
        }

        match item {
            RolloutItem::SessionMeta(_) => {}
            RolloutItem::EventMsg(EventMsg::ThreadRolledBack(rollback)) => {
                pending_rollback_turns = pending_rollback_turns
                    .saturating_add(usize::try_from(rollback.num_turns).unwrap_or(usize::MAX));
            }
            RolloutItem::EventMsg(EventMsg::TurnStarted(event)) => {
                if active_segment.as_ref().is_some_and(|active_segment| {
                    turn_ids_are_compatible(
                        active_segment.turn_id.as_deref(),
                        Some(event.turn_id.as_str()),
                    )
                }) && let Some(active_segment) = active_segment.take()
                {
                    finalize_active_segment(
                        active_segment,
                        &mut replay_slice,
                        &mut previous_model,
                        &mut reference_context_item,
                        &mut pending_rollback_turns,
                    );
                }
            }
            RolloutItem::ResponseItem(_)
            | RolloutItem::Compacted(_)
            | RolloutItem::TurnContext(_)
            | RolloutItem::EventMsg(_) => {
                let active_segment =
                    active_segment.get_or_insert_with(ActiveReplaySegment::default);
                if active_segment.newest_rollout_index.is_none() {
                    active_segment.newest_rollout_index = Some(item_index);
                }
                active_segment.oldest_rollout_index = Some(item_index);
                match item {
                    RolloutItem::Compacted(compacted) => {
                        if matches!(
                            active_segment.reference_context_item,
                            TurnReferenceContextItem::NeverSet
                        ) {
                            active_segment.reference_context_item =
                                TurnReferenceContextItem::Cleared;
                        }
                        if active_segment.base_replacement_history.is_none()
                            && let Some(replacement_history) = &compacted.replacement_history
                        {
                            active_segment.base_replacement_history =
                                Some(replacement_history.clone());
                        }
                    }
                    RolloutItem::TurnContext(ctx) => {
                        if active_segment.turn_id.is_none() {
                            active_segment.turn_id = ctx.turn_id.clone();
                        }
                        if turn_ids_are_compatible(
                            active_segment.turn_id.as_deref(),
                            ctx.turn_id.as_deref(),
                        ) {
                            active_segment.previous_model = Some(ctx.model.clone());
                            if matches!(
                                active_segment.reference_context_item,
                                TurnReferenceContextItem::NeverSet
                            ) {
                                active_segment.reference_context_item =
                                    TurnReferenceContextItem::Latest(Box::new(ctx.clone()));
                            }
                        }
                    }
                    RolloutItem::EventMsg(EventMsg::TurnComplete(event)) => {
                        if active_segment.turn_id.is_none() {
                            active_segment.turn_id = Some(event.turn_id.clone());
                        }
                    }
                    RolloutItem::EventMsg(EventMsg::TurnAborted(event)) => {
                        if active_segment.turn_id.is_none()
                            && let Some(turn_id) = &event.turn_id
                        {
                            active_segment.turn_id = Some(turn_id.clone());
                        }
                    }
                    RolloutItem::EventMsg(EventMsg::UserMessage(_)) => {
                        active_segment.counts_as_user_turn = true;
                    }
                    RolloutItem::ResponseItem(_) | RolloutItem::EventMsg(_) => {}
                    RolloutItem::SessionMeta(_) => {
                        unreachable!(
                            "session meta and rollback events are handled outside active segments"
                        )
                    }
                }
            }
        }
    }

    if let Some(active_segment) = active_segment.take() {
        finalize_active_segment(
            active_segment,
            &mut replay_slice,
            &mut previous_model,
            &mut reference_context_item,
            &mut pending_rollback_turns,
        );
    }

    let reference_context_item = match reference_context_item {
        TurnReferenceContextItem::NeverSet | TurnReferenceContextItem::Cleared => None,
        TurnReferenceContextItem::Latest(turn_reference_context_item) => {
            Some(*turn_reference_context_item)
        }
    };

    let mut replay_slice = replay_slice.unwrap_or(ReplaySlice {
        base_history: Vec::new(),
        rollout_suffix_start: source.start_index(),
        reverse_resume_index: source.start_index(),
        previous_model: None,
        reference_context_item: None,
    });
    replay_slice.previous_model = previous_model;
    replay_slice.reference_context_item = reference_context_item;
    replay_slice
}

fn reconstruct_history_until(
    turn_context: &TurnContext,
    source: &InMemoryReverseRolloutSource,
    replay_slice: &ReplaySlice,
    end_index: RolloutIndex,
) -> RolloutReconstruction {
    let mut history = ContextManager::new();
    let mut saw_legacy_compaction_without_replacement_history = false;
    history.replace(replay_slice.base_history.clone());

    for (rollout_index, item) in source.iter_forward_from(replay_slice.rollout_suffix_start) {
        if rollout_index.0 >= end_index.0 {
            break;
        }
        match item {
            RolloutItem::ResponseItem(response_item) => {
                history.record_items(
                    std::iter::once(response_item),
                    turn_context.truncation_policy,
                );
            }
            RolloutItem::Compacted(compacted) => {
                if let Some(replacement_history) = &compacted.replacement_history {
                    history.replace(replacement_history.clone());
                } else {
                    saw_legacy_compaction_without_replacement_history = true;
                    let user_messages = collect_user_messages(history.raw_items());
                    let rebuilt = compact::build_compacted_history(
                        Vec::new(),
                        &user_messages,
                        &compacted.message,
                    );
                    history.replace(rebuilt);
                }
            }
            RolloutItem::EventMsg(EventMsg::ThreadRolledBack(rollback)) => {
                history.drop_last_n_user_turns(rollback.num_turns);
            }
            RolloutItem::EventMsg(_)
            | RolloutItem::TurnContext(_)
            | RolloutItem::SessionMeta(_) => {}
        }
    }

    let reference_context_item = if saw_legacy_compaction_without_replacement_history {
        None
    } else {
        replay_slice.reference_context_item.clone()
    };

    RolloutReconstruction {
        history: history.raw_items().to_vec(),
        previous_model: replay_slice.previous_model.clone(),
        reference_context_item,
    }
}

impl Session {
    pub(super) async fn reconstruct_history_from_rollout(
        &self,
        turn_context: &TurnContext,
        rollout_items: &[RolloutItem],
    ) -> RolloutReconstruction {
        let reconstruction_state = RolloutReconstructionState::new(rollout_items.to_vec());
        self.reconstruct_history_from_rollout_state(turn_context, &reconstruction_state)
            .await
    }

    pub(super) async fn reconstruct_history_from_rollout_state(
        &self,
        turn_context: &TurnContext,
        reconstruction_state: &RolloutReconstructionState,
    ) -> RolloutReconstruction {
        reconstruction_state
            .reconstruct_history(turn_context, reconstruction_state.source.end_index())
    }
}
