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
    // Exclusive end of the currently visible rollout suffix. Backtracking moves this boundary
    // left when the newest surviving visible user turn is dropped.
    visible_end_index: RolloutIndex,
}

impl InMemoryReverseRolloutSource {
    fn new(rollout_items: Vec<RolloutItem>) -> Self {
        let startup_rollout_len =
            i64::try_from(rollout_items.len()).expect("rollout length should fit in i64");
        Self {
            rollout_items,
            startup_rollout_len,
            visible_end_index: RolloutIndex(0),
        }
    }

    fn start_index(&self) -> RolloutIndex {
        RolloutIndex(-self.startup_rollout_len)
    }

    fn iter_forward_from(
        &self,
        start: RolloutIndex,
    ) -> impl Iterator<Item = (RolloutIndex, &RolloutItem)> + '_ {
        let start = self.actual_index_from_rollout_index(start);
        let end = self.actual_index_from_rollout_index(self.visible_end_index);
        self.rollout_items[start..end]
            .iter()
            .enumerate()
            .map(move |(offset, item)| {
                let offset = i64::try_from(offset).expect("offset should fit in i64");
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
                let actual_index =
                    i64::try_from(actual_index).expect("actual index should fit in i64");
                (RolloutIndex(actual_index - self.startup_rollout_len), item)
            })
    }

    fn actual_index_from_rollout_index(&self, rollout_index: RolloutIndex) -> usize {
        usize::try_from(rollout_index.0 + self.startup_rollout_len)
            .expect("rollout index should map to a loaded rollout row")
    }
}

#[derive(Clone, Debug)]
enum HistoryBase {
    StartOfFile,
    // The current history view starts from a replacement-history checkpoint. The checkpoint
    // rollout items themselves stay in the reverse source's loaded window; replay only needs to
    // remember where forward materialization should resume after that compacted turn.
    CompactionReplacement {
        replacement_history: Vec<ResponseItem>,
        rollout_suffix_start: RolloutIndex,
    },
}

#[derive(Clone, Debug)]
pub(super) struct RolloutReconstructionState {
    source: InMemoryReverseRolloutSource,
    history_base: HistoryBase,
    previous_model: Option<String>,
    reference_context_item: Option<TurnContextItem>,
}

impl RolloutReconstructionState {
    pub(super) fn new(rollout_items: Vec<RolloutItem>) -> Self {
        let mut reconstruction_state = Self {
            source: InMemoryReverseRolloutSource::new(rollout_items),
            history_base: HistoryBase::StartOfFile,
            previous_model: None,
            reference_context_item: None,
        };
        reconstruction_state.rebuild(0);
        reconstruction_state
    }

    pub(super) fn apply_backtracking(&mut self, additional_user_turns: u32) {
        self.rebuild(additional_user_turns);
    }

    fn rebuild(&mut self, additional_user_turns: u32) {
        // Re-canonicalize the replay state from the currently visible rollout suffix plus any
        // older items needed to recover metadata or an earlier compaction base. Starting the
        // reverse scan from the current visible end makes repeated backtracking relative to the
        // current reconstructed history instead of the newest startup rollout rows.
        let mut new_history_base = None;
        let mut visible_end_index = self.source.visible_end_index;
        let mut previous_model = None;
        let mut reference_context_item = TurnReferenceContextItem::NeverSet;
        let mut pending_rollback_turns =
            usize::try_from(additional_user_turns).unwrap_or(usize::MAX);
        let mut active_segment: Option<ActiveReplaySegment> = None;

        for (item_index, item) in self.source.iter_reverse_from(self.source.visible_end_index) {
            if active_segment.is_none()
                && pending_rollback_turns == 0
                && new_history_base.is_some()
                && previous_model.is_some()
                && !matches!(reference_context_item, TurnReferenceContextItem::NeverSet)
            {
                break;
            }

            match item {
                RolloutItem::SessionMeta(_) => {}
                RolloutItem::EventMsg(EventMsg::ThreadRolledBack(rollback)) => {
                    // Historical rollback markers are applied eagerly while rebuilding state and
                    // are replayed from the same source as every other rollout item.
                    pending_rollback_turns = pending_rollback_turns
                        .saturating_add(usize::try_from(rollback.num_turns).unwrap_or(usize::MAX));
                }
                RolloutItem::EventMsg(EventMsg::TurnStarted(event)) => {
                    if active_segment.as_ref().is_some_and(|active_segment| {
                        turn_ids_are_compatible(
                            active_segment.turn_id.as_deref(),
                            Some(event.turn_id.as_str()),
                        )
                    }) {
                        if let Some(active_segment) = active_segment.take() {
                            finalize_active_segment(
                                active_segment,
                                &mut new_history_base,
                                &mut visible_end_index,
                                &mut previous_model,
                                &mut reference_context_item,
                                &mut pending_rollback_turns,
                            );
                        }
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
                &mut new_history_base,
                &mut visible_end_index,
                &mut previous_model,
                &mut reference_context_item,
                &mut pending_rollback_turns,
            );
        }

        let history_base = new_history_base.unwrap_or(HistoryBase::StartOfFile);
        let reference_context_item = match reference_context_item {
            TurnReferenceContextItem::NeverSet | TurnReferenceContextItem::Cleared => None,
            TurnReferenceContextItem::Latest(turn_reference_context_item) => {
                Some(*turn_reference_context_item)
            }
        };

        self.history_base = history_base;
        self.source.visible_end_index = visible_end_index;
        self.previous_model = previous_model;
        self.reference_context_item = reference_context_item;
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
    history_base: &mut Option<HistoryBase>,
    visible_end_index: &mut RolloutIndex,
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
            let oldest_rollout_index = active_segment
                .oldest_rollout_index
                .expect("active replay segment should contain rollout items");
            *visible_end_index = oldest_rollout_index;
        }
        return;
    }

    let ActiveReplaySegment {
        newest_rollout_index,
        oldest_rollout_index: _,
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

    if history_base.is_none()
        && let Some(replacement_history) = base_replacement_history
    {
        let newest_rollout_index =
            newest_rollout_index.expect("active replay segment should contain rollout items");
        *history_base = Some(HistoryBase::CompactionReplacement {
            replacement_history,
            rollout_suffix_start: RolloutIndex(newest_rollout_index.0 + 1),
        });
        return;
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
        let mut history = ContextManager::new();
        let mut saw_legacy_compaction_without_replacement_history = false;

        let rollout_suffix_start = match &reconstruction_state.history_base {
            HistoryBase::StartOfFile => reconstruction_state.source.start_index(),
            HistoryBase::CompactionReplacement {
                replacement_history,
                rollout_suffix_start,
                ..
            } => {
                history.replace(replacement_history.clone());
                *rollout_suffix_start
            }
        };

        for (_, item) in reconstruction_state
            .source
            .iter_forward_from(rollout_suffix_start)
        {
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
                        // Legacy rollouts without `replacement_history` should rebuild the
                        // historical TurnContext at the correct insertion point from persisted
                        // `TurnContextItem`s. These are rare enough that we currently just clear
                        // `reference_context_item`, reinject canonical context at the end of the
                        // resumed conversation, and accept the temporary out-of-distribution
                        // prompt shape.
                        // If we eventually drop support for None replacement_history compaction
                        // items, we can remove this legacy branch and build `history` directly in
                        // the first replay loop.
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
            reconstruction_state.reference_context_item.clone()
        };

        RolloutReconstruction {
            history: history.raw_items().to_vec(),
            previous_model: reconstruction_state.previous_model.clone(),
            reference_context_item,
        }
    }
}
