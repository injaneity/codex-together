use std::collections::HashMap;
use std::collections::HashSet;
use std::time::Duration;
use std::time::Instant;

use codex_together_protocol::ConnectedMember;
use codex_together_protocol::TogetherRole;
use crossterm::event::KeyCode;
use crossterm::event::KeyEvent;
use crossterm::event::KeyModifiers;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Stylize;
use ratatui::text::Line;
use ratatui::text::Span;
use ratatui::widgets::Paragraph;
use ratatui::widgets::Widget;
use ratatui::widgets::Wrap;
use textwrap::wrap;

use super::CancellationEvent;
use super::bottom_pane_view::BottomPaneView;
use crate::render::renderable::Renderable;

pub(crate) const TOGETHER_CENTER_VIEW_ID: &str = "together-center";

const MAX_LABEL_WIDTH: usize = 14;
const SPRITE_HEIGHT: usize = 3;
const BLANK_SPRITE: &str = "         ";
const LANDING_FINAL_FRAME: usize = SPRITE_HEIGHT - 1;
const STEPOUT_FINAL_FRAME: usize = SPRITE_HEIGHT;
const ANIMATION_FRAME_MS: u64 = 90;

const UNICODE_SPRITES: [[&str; SPRITE_HEIGHT]; 7] = [
    [" (•.•)/  ", "<)   )   ", " /   \\   "],
    ["\\(•.•)/  ", " (   )   ", " /   \\   "],
    [" (•.•)/  ", "<)   )   ", " |   /   "],
    [" \\(•.•)  ", "  (   )> ", "  /   \\  "],
    [" ᕦ(•.•)ᕤ ", "  (   )  ", "  /   \\  "],
    ["\\(°o°)/  ", " (   )   ", " /   \\   "],
    [" (•_•)   ", "|(   )>  ", " /   \\   "],
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TogetherPresenceState {
    Connected,
    Reconnecting,
    Stale,
    Disconnected,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CrewAnimationState {
    Idle,
    Landing(usize),
    SteppingOut(usize),
}

#[derive(Debug, Clone)]
struct CrewSlot {
    animation: CrewAnimationState,
}

pub(crate) struct TogetherCenterViewParams {
    pub(crate) state: TogetherPresenceState,
    pub(crate) owner_email: Option<String>,
    pub(crate) endpoint: Option<String>,
    pub(crate) connected_members: Vec<ConnectedMember>,
    pub(crate) max_visible: usize,
    pub(crate) mask_email_labels: bool,
    pub(crate) mascot_enabled: bool,
    pub(crate) motion_enabled: bool,
    pub(crate) landing_emails: Vec<String>,
    pub(crate) departing_members: Vec<ConnectedMember>,
}

pub(crate) struct TogetherCenterView {
    state: TogetherPresenceState,
    owner_email: Option<String>,
    endpoint: Option<String>,
    connected_members: Vec<ConnectedMember>,
    crew_slots: Vec<CrewSlot>,
    max_visible: usize,
    mask_email_labels: bool,
    mascot_enabled: bool,
    motion_enabled: bool,
    show_all_members: bool,
    complete: bool,
    frame_interval: Duration,
    last_animation_tick: Instant,
}

impl TogetherCenterView {
    pub(crate) fn new(params: TogetherCenterViewParams) -> Self {
        let TogetherCenterViewParams {
            state,
            owner_email,
            endpoint,
            mut connected_members,
            max_visible,
            mask_email_labels,
            mascot_enabled,
            motion_enabled,
            landing_emails,
            mut departing_members,
        } = params;

        connected_members.sort_by(|a, b| a.email.cmp(&b.email));
        departing_members.sort_by(|a, b| a.email.cmp(&b.email));
        let landing_set: HashSet<String> = landing_emails.into_iter().collect();
        let connected_email_set: HashSet<String> = connected_members
            .iter()
            .map(|member| member.email.clone())
            .collect();

        let mut crew_slots = connected_members
            .iter()
            .map(|member| {
                let animation = if motion_enabled && landing_set.contains(&member.email) {
                    CrewAnimationState::Landing(0)
                } else {
                    CrewAnimationState::Idle
                };
                CrewSlot { animation }
            })
            .collect::<Vec<_>>();

        if motion_enabled {
            for member in departing_members {
                if connected_email_set.contains(&member.email) {
                    continue;
                }
                crew_slots.push(CrewSlot {
                    animation: CrewAnimationState::SteppingOut(0),
                });
            }
        }

        Self {
            state,
            owner_email,
            endpoint,
            connected_members,
            crew_slots,
            max_visible: max_visible.max(1),
            mask_email_labels,
            mascot_enabled,
            motion_enabled,
            show_all_members: false,
            complete: false,
            frame_interval: Duration::from_millis(ANIMATION_FRAME_MS),
            last_animation_tick: Instant::now(),
        }
    }

    fn visible_connected_count(&self) -> usize {
        self.connected_members.len().min(self.max_visible)
    }

    fn overflow_count(&self) -> usize {
        self.connected_members
            .len()
            .saturating_sub(self.visible_connected_count())
    }

    fn visible_slots(&self) -> Vec<&CrewSlot> {
        self.crew_slots.iter().take(self.max_visible).collect()
    }

    fn has_active_animation(&self) -> bool {
        self.crew_slots.iter().any(|slot| {
            !matches!(slot.animation, CrewAnimationState::Idle)
                && self.mascot_enabled
                && self.motion_enabled
        })
    }

    fn step_animation(&mut self) {
        self.crew_slots.retain_mut(|slot| match slot.animation {
            CrewAnimationState::Idle => true,
            CrewAnimationState::Landing(frame) => {
                if frame >= LANDING_FINAL_FRAME {
                    slot.animation = CrewAnimationState::Idle;
                } else {
                    slot.animation = CrewAnimationState::Landing(frame + 1);
                }
                true
            }
            CrewAnimationState::SteppingOut(frame) => {
                if frame >= STEPOUT_FINAL_FRAME {
                    return false;
                }
                slot.animation = CrewAnimationState::SteppingOut(frame + 1);
                true
            }
        });
    }

    fn connection_status_line(&self) -> String {
        match self.state {
            TogetherPresenceState::Connected => match self.owner_email.as_deref() {
                Some(owner) => format!("Status: connected  owner={owner}"),
                None => "Status: connected".to_string(),
            },
            TogetherPresenceState::Reconnecting => "Status: reconnecting".to_string(),
            TogetherPresenceState::Stale => "Status: stale".to_string(),
            TogetherPresenceState::Disconnected => "Status: disconnected".to_string(),
        }
    }

    fn crew_sprites(&self) -> Vec<Line<'static>> {
        if !self.mascot_enabled {
            return vec!["[ mascot disabled ]".dim().into()];
        }

        let slots = self.visible_slots();
        if slots.is_empty() {
            return vec!["[ no crew connected ]".dim().into()];
        }

        (0..SPRITE_HEIGHT)
            .map(|line_idx| {
                let mut spans = Vec::new();
                for (slot_idx, slot) in slots.iter().enumerate() {
                    if slot_idx > 0 {
                        spans.push("  ".into());
                    }
                    let sprite =
                        sprite_line_for_state(slot_idx, slot.animation, line_idx, self.state);
                    spans.push(style_slot(sprite, slot_idx, self.state, slot.animation));
                }
                Line::from(spans)
            })
            .collect()
    }

    fn summary_lines(&self, width: usize) -> Vec<Line<'static>> {
        let mut out = Vec::new();
        let labels = render_member_labels(
            &self.connected_members,
            self.mask_email_labels,
            MAX_LABEL_WIDTH,
        );
        let visible = self.visible_connected_count();
        for index in 0..visible {
            let member = &self.connected_members[index];
            out.push(
                format!(
                    "{}. {} {}",
                    index + 1,
                    role_badge(member.role),
                    labels[index]
                )
                .into(),
            );
        }

        let overflow = self.overflow_count();
        if overflow > 0 {
            for row in wrap(
                &format!("+{overflow} more member(s). Press `v` to toggle full roster."),
                width.max(1),
            ) {
                out.push(row.into_owned().dim().into());
            }
        }
        out
    }

    fn full_roster_lines(&self) -> Vec<Line<'static>> {
        let labels = render_member_labels(
            &self.connected_members,
            self.mask_email_labels,
            MAX_LABEL_WIDTH,
        );
        self.connected_members
            .iter()
            .zip(labels)
            .enumerate()
            .map(|(index, (member, label))| {
                format!("{}. {} {}", index + 1, role_badge(member.role), label).into()
            })
            .collect()
    }

    fn render_lines(&self, width: usize) -> Vec<Line<'static>> {
        let mut lines = vec!["Together Center".bold().into()];
        for row in wrap(&self.connection_status_line(), width.max(1)) {
            lines.push(match self.state {
                TogetherPresenceState::Connected => row.into_owned().cyan().into(),
                TogetherPresenceState::Reconnecting => row.into_owned().yellow().into(),
                TogetherPresenceState::Stale => row.into_owned().yellow().dim().into(),
                TogetherPresenceState::Disconnected => row.into_owned().dim().into(),
            });
        }

        if let Some(endpoint) = self.endpoint.as_deref() {
            for row in wrap(&format!("Endpoint: {endpoint}"), width.max(1)) {
                lines.push(row.into_owned().dim().into());
            }
        }

        lines.push("".into());
        lines.extend(self.crew_sprites());
        lines.push("".into());
        if !self.motion_enabled {
            lines.push("motion: off".dim().into());
            lines.push("".into());
        }

        if self.show_all_members {
            lines.push("Full Roster".bold().into());
            lines.extend(self.full_roster_lines());
        } else {
            lines.push("Visible Crew".bold().into());
            lines.extend(self.summary_lines(width));
        }

        lines.push("".into());
        let mut hint = "v toggle full roster".to_string();
        if self.overflow_count() == 0 {
            hint = "esc close".to_string();
        } else {
            hint.push_str(" · esc close");
        }
        lines.push(hint.dim().into());
        lines
    }
}

impl BottomPaneView for TogetherCenterView {
    fn handle_key_event(&mut self, key_event: KeyEvent) {
        match key_event {
            KeyEvent {
                code: KeyCode::Esc, ..
            } => {
                let _ = self.on_ctrl_c();
            }
            KeyEvent {
                code: KeyCode::Char('v'),
                modifiers: KeyModifiers::NONE,
                ..
            } => {
                if self.overflow_count() > 0 {
                    self.show_all_members = !self.show_all_members;
                }
            }
            _ => {}
        }
    }

    fn view_id(&self) -> Option<&'static str> {
        Some(TOGETHER_CENTER_VIEW_ID)
    }

    fn on_ctrl_c(&mut self) -> CancellationEvent {
        self.complete = true;
        CancellationEvent::Handled
    }

    fn is_complete(&self) -> bool {
        self.complete
    }

    fn pre_draw_tick(&mut self) -> Option<Duration> {
        if !self.has_active_animation() {
            return None;
        }

        let now = Instant::now();
        let elapsed = now.saturating_duration_since(self.last_animation_tick);
        if elapsed < self.frame_interval {
            return Some(self.frame_interval - elapsed);
        }

        self.last_animation_tick = now;
        self.step_animation();
        if self.has_active_animation() {
            Some(self.frame_interval)
        } else {
            None
        }
    }
}

impl Renderable for TogetherCenterView {
    fn render(&self, area: Rect, buf: &mut Buffer) {
        if area.width == 0 || area.height == 0 {
            return;
        }

        let lines = self.render_lines(area.width as usize);
        Paragraph::new(lines)
            .wrap(Wrap { trim: false })
            .render(area, buf);
    }

    fn desired_height(&self, width: u16) -> u16 {
        let lines = self.render_lines(width as usize);
        lines.len().min(u16::MAX as usize) as u16
    }
}

fn style_slot(
    text: &'static str,
    slot_idx: usize,
    state: TogetherPresenceState,
    animation: CrewAnimationState,
) -> Span<'static> {
    let base = match slot_idx % 5 {
        0 => text.cyan(),
        1 => text.green(),
        2 => text.magenta(),
        3 => text.yellow(),
        _ => text.blue(),
    };
    match (state, animation) {
        (TogetherPresenceState::Connected, CrewAnimationState::Idle) => base.bold(),
        (TogetherPresenceState::Connected, CrewAnimationState::Landing(_)) => base.bold(),
        (TogetherPresenceState::Connected, CrewAnimationState::SteppingOut(_)) => base.dim(),
        (TogetherPresenceState::Reconnecting, _) => base.dim(),
        (TogetherPresenceState::Stale, _) => base.dim().italic(),
        (TogetherPresenceState::Disconnected, _) => base.dim(),
    }
}

fn sprite_line_for_state(
    slot_idx: usize,
    animation: CrewAnimationState,
    line_idx: usize,
    state: TogetherPresenceState,
) -> &'static str {
    let base = UNICODE_SPRITES[slot_idx % UNICODE_SPRITES.len()];
    match animation {
        CrewAnimationState::Idle => match state {
            TogetherPresenceState::Connected => base[line_idx],
            TogetherPresenceState::Reconnecting => sprite_line_for_landing(base, 1, line_idx),
            TogetherPresenceState::Stale => sprite_line_for_stepout(base, 1, line_idx),
            TogetherPresenceState::Disconnected => BLANK_SPRITE,
        },
        CrewAnimationState::Landing(frame) => sprite_line_for_landing(base, frame, line_idx),
        CrewAnimationState::SteppingOut(frame) => sprite_line_for_stepout(base, frame, line_idx),
    }
}

fn sprite_line_for_landing(
    base: [&'static str; SPRITE_HEIGHT],
    frame: usize,
    line_idx: usize,
) -> &'static str {
    let clamped = frame.min(LANDING_FINAL_FRAME);
    let visible_from = (SPRITE_HEIGHT - 1).saturating_sub(clamped);
    if line_idx >= visible_from {
        base[line_idx]
    } else {
        BLANK_SPRITE
    }
}

fn sprite_line_for_stepout(
    base: [&'static str; SPRITE_HEIGHT],
    frame: usize,
    line_idx: usize,
) -> &'static str {
    let clamped = frame.min(STEPOUT_FINAL_FRAME);
    let keep_until = SPRITE_HEIGHT.saturating_sub(clamped);
    if line_idx < keep_until {
        base[line_idx]
    } else {
        BLANK_SPRITE
    }
}

fn role_badge(role: TogetherRole) -> &'static str {
    match role {
        TogetherRole::Owner => "[O]",
        TogetherRole::Member => "[M]",
    }
}

fn render_member_labels(
    members: &[ConnectedMember],
    mask_email_labels: bool,
    max_width: usize,
) -> Vec<String> {
    let base_labels: Vec<String> = members
        .iter()
        .map(|member| {
            if mask_email_labels {
                masked_email(&member.email)
            } else {
                member.email.clone()
            }
        })
        .collect();

    let mut counts: HashMap<String, usize> = HashMap::new();
    for label in &base_labels {
        *counts.entry(label.clone()).or_insert(0) += 1;
    }

    let mut seen: HashMap<String, usize> = HashMap::new();
    members
        .iter()
        .zip(base_labels)
        .map(|(member, label)| {
            let current = seen.entry(label.clone()).or_insert(0);
            *current += 1;

            let mut candidate = if counts.get(&label).copied().unwrap_or(0) > 1 && *current > 1 {
                format!("{label}#{}", *current)
            } else {
                label
            };

            if candidate.chars().count() > max_width {
                candidate = fit_with_hash(&candidate, &member.email, max_width);
            }
            candidate
        })
        .collect()
}

fn masked_email(email: &str) -> String {
    let (local, domain) = email.split_once('@').unwrap_or((email, ""));
    let local_mask = if local.chars().count() <= 2 {
        format!("{local}***")
    } else {
        let prefix: String = local.chars().take(2).collect();
        format!("{prefix}***")
    };
    let domain_prefix = domain.chars().next().unwrap_or('?');
    format!("{local_mask}@{domain_prefix}…")
}

fn fit_with_hash(value: &str, seed: &str, max_width: usize) -> String {
    if max_width == 0 {
        return String::new();
    }
    if value.chars().count() <= max_width {
        return value.to_string();
    }

    let hash = short_hash(seed);
    if max_width <= hash.len() {
        return hash.chars().take(max_width).collect();
    }

    let keep = max_width.saturating_sub(hash.len() + 1);
    let prefix: String = value.chars().take(keep).collect();
    format!("{prefix}~{hash}")
}

fn short_hash(input: &str) -> String {
    let mut acc: u32 = 0;
    for byte in input.as_bytes() {
        acc = acc.wrapping_mul(33).wrapping_add(u32::from(*byte));
    }
    format!("{:02x}", acc & 0xff)
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    fn member(email: &str, role: TogetherRole) -> ConnectedMember {
        ConnectedMember {
            email: email.to_string(),
            role,
        }
    }

    #[test]
    fn masked_email_is_stable_and_compact() {
        assert_eq!(masked_email("alice@example.com"), "al***@e…");
        assert_eq!(masked_email("ab@example.com"), "ab***@e…");
    }

    #[test]
    fn duplicate_labels_get_deterministic_suffixes() {
        let members = vec![
            member("alice@example.com", TogetherRole::Member),
            member("alice@elsewhere.com", TogetherRole::Member),
            member("alice@example.com", TogetherRole::Owner),
        ];
        let labels = render_member_labels(&members, true, MAX_LABEL_WIDTH);
        assert_eq!(labels[0], "al***@e…");
        assert_eq!(labels[1], "al***@e…#2");
        assert_eq!(labels[2], "al***@e…#3");
    }

    #[test]
    fn labels_fall_back_to_hash_when_width_is_tight() {
        let members = vec![member("very.long.user@example.com", TogetherRole::Member)];
        let labels = render_member_labels(&members, false, 8);
        assert_eq!(labels[0].chars().count(), 8);
        assert!(labels[0].contains('~'));
    }

    #[test]
    fn landing_animation_transitions_to_idle() {
        let mut view = TogetherCenterView::new(TogetherCenterViewParams {
            state: TogetherPresenceState::Connected,
            owner_email: None,
            endpoint: None,
            connected_members: vec![member("alice@example.com", TogetherRole::Member)],
            max_visible: 5,
            mask_email_labels: true,
            mascot_enabled: true,
            motion_enabled: true,
            landing_emails: vec!["alice@example.com".to_string()],
            departing_members: Vec::new(),
        });
        assert!(matches!(
            view.crew_slots[0].animation,
            CrewAnimationState::Landing(0)
        ));
        for _ in 0..=LANDING_FINAL_FRAME {
            view.step_animation();
        }
        assert!(matches!(
            view.crew_slots[0].animation,
            CrewAnimationState::Idle
        ));
    }

    #[test]
    fn stepout_animation_removes_departing_member() {
        let mut view = TogetherCenterView::new(TogetherCenterViewParams {
            state: TogetherPresenceState::Connected,
            owner_email: None,
            endpoint: None,
            connected_members: Vec::new(),
            max_visible: 5,
            mask_email_labels: true,
            mascot_enabled: true,
            motion_enabled: true,
            landing_emails: Vec::new(),
            departing_members: vec![member("alice@example.com", TogetherRole::Member)],
        });
        assert_eq!(view.crew_slots.len(), 1);
        for _ in 0..=STEPOUT_FINAL_FRAME {
            view.step_animation();
        }
        assert!(view.crew_slots.is_empty());
    }
}
