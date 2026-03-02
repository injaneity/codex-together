use std::collections::HashMap;

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

const MAX_LABEL_WIDTH: usize = 14;

const UNICODE_SPRITES: [[&str; 4]; 5] = [
    [" ▗▆▖ ", "▐█▀▌", "▐█▄▌", " ▝▀▘ "],
    [" ▟█▙ ", "▐▛█▌", "▐▙█▌", " ▝▀▘ "],
    [" ▗▄▖ ", "▐▙█▌", "▐▛█▌", " ▝▀▘ "],
    [" ▗▇▖ ", "▐█▚▌", "▐█▞▌", " ▝▀▘ "],
    [" ▗▅▖ ", "▐▜█▌", "▐▟█▌", " ▝▀▘ "],
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)]
pub(crate) enum TogetherPresenceState {
    Connected,
    Reconnecting,
    Stale,
    Disconnected,
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
}

pub(crate) struct TogetherCenterView {
    state: TogetherPresenceState,
    owner_email: Option<String>,
    endpoint: Option<String>,
    connected_members: Vec<ConnectedMember>,
    max_visible: usize,
    mask_email_labels: bool,
    mascot_enabled: bool,
    motion_enabled: bool,
    show_all_members: bool,
    complete: bool,
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
        } = params;
        connected_members.sort_by(|a, b| a.email.cmp(&b.email));
        Self {
            state,
            owner_email,
            endpoint,
            connected_members,
            max_visible: max_visible.max(1),
            mask_email_labels,
            mascot_enabled,
            motion_enabled,
            show_all_members: false,
            complete: false,
        }
    }

    fn visible_count(&self) -> usize {
        self.connected_members.len().min(self.max_visible)
    }

    fn overflow_count(&self) -> usize {
        self.connected_members
            .len()
            .saturating_sub(self.visible_count())
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

        let visible = self.visible_count();
        if visible == 0 {
            return vec!["[ no crew connected ]".dim().into()];
        }

        (0..4)
            .map(|line_idx| {
                let mut spans = Vec::new();
                for slot_idx in 0..visible {
                    if slot_idx > 0 {
                        spans.push("  ".into());
                    }
                    spans.push(styled_sprite(
                        UNICODE_SPRITES[slot_idx % UNICODE_SPRITES.len()][line_idx],
                        slot_idx,
                    ));
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
        let visible = self.visible_count();
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
            lines.push(row.into_owned().cyan().into());
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

    fn on_ctrl_c(&mut self) -> CancellationEvent {
        self.complete = true;
        CancellationEvent::Handled
    }

    fn is_complete(&self) -> bool {
        self.complete
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

fn styled_sprite(text: &'static str, slot_idx: usize) -> Span<'static> {
    match slot_idx % 5 {
        0 => text.cyan().bold(),
        1 => text.green().bold(),
        2 => text.magenta().bold(),
        3 => text.yellow().bold(),
        _ => text.blue().bold(),
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

    #[test]
    fn masked_email_is_stable_and_compact() {
        assert_eq!(masked_email("alice@example.com"), "al***@e…");
        assert_eq!(masked_email("ab@example.com"), "ab***@e…");
    }

    #[test]
    fn duplicate_labels_get_deterministic_suffixes() {
        let members = vec![
            ConnectedMember {
                email: "alice@example.com".to_string(),
                role: TogetherRole::Member,
            },
            ConnectedMember {
                email: "alice@elsewhere.com".to_string(),
                role: TogetherRole::Member,
            },
            ConnectedMember {
                email: "alice@example.com".to_string(),
                role: TogetherRole::Owner,
            },
        ];
        let labels = render_member_labels(&members, true, MAX_LABEL_WIDTH);
        assert_eq!(labels[0], "al***@e…");
        assert_eq!(labels[1], "al***@e…#2");
        assert_eq!(labels[2], "al***@e…#3");
    }

    #[test]
    fn labels_fall_back_to_hash_when_width_is_tight() {
        let members = vec![ConnectedMember {
            email: "very.long.user@example.com".to_string(),
            role: TogetherRole::Member,
        }];
        let labels = render_member_labels(&members, false, 8);
        assert_eq!(labels[0].chars().count(), 8);
        assert!(labels[0].contains('~'));
    }
}
