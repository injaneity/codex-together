//! Shared helpers for filtering and matching built-in slash commands.
//!
//! The same sandbox- and feature-gating rules are used by both the composer
//! and the command popup. Centralizing them here keeps those call sites small
//! and ensures they stay in sync.
use codex_utils_fuzzy_match::fuzzy_match;

use crate::slash_command::SlashCommand;
use crate::slash_command::built_in_slash_commands;

/// Return the built-ins that should be visible/usable for the current input.
pub(crate) fn builtins_for_input(
    collaboration_modes_enabled: bool,
    connectors_enabled: bool,
    personality_command_enabled: bool,
    realtime_conversation_enabled: bool,
    audio_device_selection_enabled: bool,
    allow_elevate_sandbox: bool,
) -> Vec<(&'static str, SlashCommand)> {
    let status = std::env::var("CODEX_TOGETHER_STATUS")
        .unwrap_or_else(|_| "disconnected".to_string())
        .to_ascii_lowercase();
    let connected = !status.trim().is_empty() && status != "disconnected";
    let hosting = status.starts_with("together host:");

    built_in_slash_commands()
        .into_iter()
        .filter(|(_, cmd)| allow_elevate_sandbox || *cmd != SlashCommand::ElevateSandbox)
        .filter(|(_, cmd)| {
            collaboration_modes_enabled
                || !matches!(*cmd, SlashCommand::Collab | SlashCommand::Plan)
        })
        .filter(|(_, cmd)| connectors_enabled || *cmd != SlashCommand::Apps)
        .filter(|(_, cmd)| personality_command_enabled || *cmd != SlashCommand::Personality)
        .filter(|(_, cmd)| realtime_conversation_enabled || *cmd != SlashCommand::Realtime)
        .filter(|(_, cmd)| audio_device_selection_enabled || *cmd != SlashCommand::Settings)
        .filter(|(_, cmd)| !connected || *cmd != SlashCommand::Host)
        .filter(|(_, cmd)| {
            connected
                || !matches!(
                    cmd,
                    SlashCommand::Leave
                        | SlashCommand::Share
                        | SlashCommand::Threads
                        | SlashCommand::History
                )
        })
        .filter(|(_, cmd)| hosting || *cmd != SlashCommand::Close)
        .collect()
}

/// Find a single built-in command by exact name, after applying the gating rules.
pub(crate) fn find_builtin_command(
    name: &str,
    collaboration_modes_enabled: bool,
    connectors_enabled: bool,
    personality_command_enabled: bool,
    realtime_conversation_enabled: bool,
    audio_device_selection_enabled: bool,
    allow_elevate_sandbox: bool,
) -> Option<SlashCommand> {
    builtins_for_input(
        collaboration_modes_enabled,
        connectors_enabled,
        personality_command_enabled,
        realtime_conversation_enabled,
        audio_device_selection_enabled,
        allow_elevate_sandbox,
    )
    .into_iter()
    .find(|(command_name, _)| *command_name == name)
    .map(|(_, cmd)| cmd)
}

/// Whether any visible built-in fuzzily matches the provided prefix.
pub(crate) fn has_builtin_prefix(
    name: &str,
    collaboration_modes_enabled: bool,
    connectors_enabled: bool,
    personality_command_enabled: bool,
    realtime_conversation_enabled: bool,
    audio_device_selection_enabled: bool,
    allow_elevate_sandbox: bool,
) -> bool {
    builtins_for_input(
        collaboration_modes_enabled,
        connectors_enabled,
        personality_command_enabled,
        realtime_conversation_enabled,
        audio_device_selection_enabled,
        allow_elevate_sandbox,
    )
    .into_iter()
    .any(|(command_name, _)| fuzzy_match(command_name, name).is_some())
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    struct TogetherStatusGuard {
        previous: Option<String>,
    }

    impl TogetherStatusGuard {
        fn set(value: &str) -> Self {
            let previous = std::env::var("CODEX_TOGETHER_STATUS").ok();
            unsafe {
                std::env::set_var("CODEX_TOGETHER_STATUS", value);
            }
            Self { previous }
        }
    }

    impl Drop for TogetherStatusGuard {
        fn drop(&mut self) {
            unsafe {
                match &self.previous {
                    Some(value) => std::env::set_var("CODEX_TOGETHER_STATUS", value),
                    None => std::env::remove_var("CODEX_TOGETHER_STATUS"),
                }
            }
        }
    }

    #[test]
    fn debug_command_still_resolves_for_dispatch() {
        let cmd = find_builtin_command("debug-config", true, true, true, false, false, false);
        assert_eq!(cmd, Some(SlashCommand::DebugConfig));
    }

    #[test]
    fn clear_command_resolves_for_dispatch() {
        assert_eq!(
            find_builtin_command("clear", true, true, true, false, false, false),
            Some(SlashCommand::Clear)
        );
    }

    #[test]
    fn realtime_command_is_hidden_when_realtime_is_disabled() {
        assert_eq!(
            find_builtin_command("realtime", true, true, true, false, true, false),
            None
        );
    }

    #[test]
    fn settings_command_is_hidden_when_realtime_is_disabled() {
        assert_eq!(
            find_builtin_command("settings", true, true, true, false, false, false),
            None
        );
    }

    #[test]
    fn settings_command_is_hidden_when_audio_device_selection_is_disabled() {
        assert_eq!(
            find_builtin_command("settings", true, true, true, true, false, false),
            None
        );
    }

    #[test]
    fn threads_hidden_when_disconnected() {
        let _guard = TogetherStatusGuard::set("disconnected");
        assert_eq!(
            find_builtin_command("threads", true, true, true, false, true, false),
            None
        );
    }

    #[test]
    fn threads_visible_when_connected() {
        let _guard = TogetherStatusGuard::set("together @owner@local");
        assert_eq!(
            find_builtin_command("threads", true, true, true, false, true, false),
            Some(SlashCommand::Threads)
        );
    }

    #[test]
    fn host_visible_when_disconnected() {
        let _guard = TogetherStatusGuard::set("disconnected");
        assert_eq!(
            find_builtin_command("host", true, true, true, false, true, false),
            Some(SlashCommand::Host)
        );
    }

    #[test]
    fn host_hidden_when_connected() {
        let _guard = TogetherStatusGuard::set("together @owner@local");
        assert_eq!(
            find_builtin_command("host", true, true, true, false, true, false),
            None
        );
    }

    #[test]
    fn leave_hidden_when_disconnected() {
        let _guard = TogetherStatusGuard::set("disconnected");
        assert_eq!(
            find_builtin_command("leave", true, true, true, false, true, false),
            None
        );
    }

    #[test]
    fn leave_visible_when_connected() {
        let _guard = TogetherStatusGuard::set("together @owner@local");
        assert_eq!(
            find_builtin_command("leave", true, true, true, false, true, false),
            Some(SlashCommand::Leave)
        );
    }

    #[test]
    fn close_hidden_when_disconnected() {
        let _guard = TogetherStatusGuard::set("disconnected");
        assert_eq!(
            find_builtin_command("close", true, true, true, false, true, false),
            None
        );
    }

    #[test]
    fn close_visible_when_connected() {
        let _guard = TogetherStatusGuard::set("together host:abc123");
        assert_eq!(
            find_builtin_command("close", true, true, true, false, true, false),
            Some(SlashCommand::Close)
        );
    }

    #[test]
    fn close_hidden_for_member() {
        let _guard = TogetherStatusGuard::set("together @owner@local");
        assert_eq!(
            find_builtin_command("close", true, true, true, false, true, false),
            None
        );
    }

    #[test]
    fn share_hidden_when_disconnected() {
        let _guard = TogetherStatusGuard::set("disconnected");
        assert_eq!(
            find_builtin_command("share", true, true, true, false, true, false),
            None
        );
    }

    #[test]
    fn share_visible_when_connected() {
        let _guard = TogetherStatusGuard::set("together @owner@local");
        assert_eq!(
            find_builtin_command("share", true, true, true, false, true, false),
            Some(SlashCommand::Share)
        );
    }
}
