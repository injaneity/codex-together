# Second Brain Prune Status

Last updated: 2026-03-16

## Summary

This file captures the exact prune work already done, the files touched, and the current verification state.

## Code already changed

### `together-protocol`

File:

- `together-protocol/src/lib.rs`

Removed from protocol:

- `METHOD_THREAD_SHARE`
- `METHOD_THREAD_LIST`
- `METHOD_THREAD_INSPECT`
- old replay/share/list/read request and response types
- old shared-thread summary/replay types
- unused member update notification constant

Kept:

- `host/start`
- `host/status`
- `host/stop`
- `session/join`
- `session/leave`
- `context/*`
- `handoff/*`
- `context/write*`

### `state`

Files:

- `state/src/model/together.rs`
- `state/src/model/mod.rs`
- `state/src/lib.rs`
- `state/src/runtime/together.rs`

Removed:

- `TogetherThreadAclRecord`
- thread fork persistence helpers
- checked-out-thread session fields
- `TogetherMemberRecord`
- state-layer `TogetherRole`
- persisted member upsert/list helpers

Kept:

- together server record
- together client session record

Important note:

- old SQL migrations were deliberately not edited to avoid migration checksum drift

### `together-server`

File:

- `together-server/src/lib.rs`

Major prune/re-center:

- removed explicit shared-thread host/list/inspect RPC handling
- context search now derives thread context from app-server `thread/list`
- removed in-memory shared thread catalog
- removed rollout/share-thread replay plumbing
- removed persisted member bookkeeping writes
- removed unused member-update notification broadcast

Behavior shift already implemented:

- thread context is now effectively shared-by-default for context browsing
- together context search combines:
  - repo context markdown files
  - app-server thread list entries scoped to the current repo

### `tui`

Files touched:

- `tui/src/slash_command.rs`
- `tui/src/chatwidget.rs`
- `tui/src/app_event.rs`
- `tui/src/app.rs`
- `tui/src/bottom_pane/chat_composer.rs`
- `tui/src/bottom_pane/mod.rs`
- `tui/src/bottom_pane/slash_commands.rs`
- `tui/src/bottom_pane/status_line_setup.rs`
- `tui/src/history_cell.rs`
- `tui/src/lib.rs`
- `tui/src/chatwidget/tests.rs`

Snapshots changed or removed:

- `tui/src/bottom_pane/snapshots/codex_tui__bottom_pane__chat_composer__tests__context_popup.snap`
- `tui/src/chatwidget/snapshots/codex_tui__chatwidget__tests__together_context_view.snap`
- deleted `tui/src/bottom_pane/snapshots/codex_tui__bottom_pane__chat_composer__tests__input_disabled_together_checkout.snap`
- deleted `tui/src/chatwidget/snapshots/codex_tui__chatwidget__tests__together_threads_view_refresh_after_delete.snap`
- `tui/src/snapshots/codex_tui__history_cell__tests__session_info_availability_nux_tooltip_snapshot.snap` also shows as modified in git status

Removed from TUI:

- `/share`
- `/threads`
- read-only together checkout/inspect flow
- checked-out-thread env plumbing
- extra Together status-line/header chrome
- thread-list refresh/open helpers
- dead helpers left over from the above flows

Further TUI copy simplification already applied:

- status/hints now prefer "server" / "participants" language over "owner" / "member"
- connected status label for non-hosts changed from `together @owner@example.com` to `together server:<id>`
- host/member labels in participant lists were removed from the user-facing text

## Files currently modified in git status

As of this handoff, `git status --short` showed:

- `state/src/lib.rs`
- `state/src/model/mod.rs`
- `state/src/model/together.rs`
- `state/src/runtime/together.rs`
- `together-protocol/src/lib.rs`
- `together-server/src/lib.rs`
- `tui/src/app.rs`
- `tui/src/app_event.rs`
- `tui/src/bottom_pane/chat_composer.rs`
- `tui/src/bottom_pane/mod.rs`
- `tui/src/bottom_pane/slash_commands.rs`
- `tui/src/bottom_pane/snapshots/codex_tui__bottom_pane__chat_composer__tests__context_popup.snap`
- deleted `tui/src/bottom_pane/snapshots/codex_tui__bottom_pane__chat_composer__tests__input_disabled_together_checkout.snap`
- `tui/src/bottom_pane/status_line_setup.rs`
- `tui/src/chatwidget.rs`
- `tui/src/chatwidget/snapshots/codex_tui__chatwidget__tests__together_context_view.snap`
- deleted `tui/src/chatwidget/snapshots/codex_tui__chatwidget__tests__together_threads_view_refresh_after_delete.snap`
- `tui/src/chatwidget/tests.rs`
- `tui/src/history_cell.rs`
- `tui/src/lib.rs`
- `tui/src/slash_command.rs`
- `tui/src/snapshots/codex_tui__history_cell__tests__session_info_availability_nux_tooltip_snapshot.snap`

## Verification already completed

These were confirmed green during this thread:

- `just fmt`
- `cargo test -p codex-state`
- `cargo test -p codex-together-protocol`
- `cargo test -p codex-together-server`

Also previously confirmed green earlier in the prune work, before the latest status-copy simplification:

- `cargo test -p codex-tui`

## Verification still unfinished at handoff time

At handoff time, a fresh `cargo test -p codex-tui` run was still in progress / unresolved because `codex-core` recompilation was taking a long time.

Most likely outcomes:

- it may pass after the long rebuild
- or it may fail only on the expected snapshot for the updated Together status hint text

Most likely snapshot to review:

- `tui/src/chatwidget/snapshots/codex_tui__chatwidget__tests__together_status_hint_with_build_identity.snap`

Potentially relevant tests changed alongside that copy:

- `tui/src/chatwidget/tests.rs`
  - `together_status_hint_includes_server_build_identity`
  - `together_exit_command_leaves_for_members`

## Recommended next commands

Run these in order:

```bash
just fmt
cargo test -p codex-state
cargo test -p codex-together-protocol
cargo test -p codex-together-server
cargo test -p codex-tui
```

If `codex-tui` only fails on intended snapshot drift:

```bash
cargo insta accept
```

Then, because this is a large Rust change, run scoped fixes:

```bash
just fix -p codex-state
just fix -p codex-together-protocol
just fix -p codex-together-server
just fix -p codex-tui
```

Per repo instructions, do not re-run tests after `just fix` / `just fmt`.

## Recommended next prune target after TUI is green

If pruning continues before graph work:

1. Audit whether host/join/status can be simplified even further while keeping repo-attached transport intact.
2. Avoid reintroducing owner/member/ACL semantics into the user-facing flow.
3. Keep `/context` and `/handoff` as the primary surfaces.
4. Do not bring back thread visibility or explicit share toggles.

## Recommended next feature work after prune is stable

Start graph + handoff implementation in this order:

1. actor model
2. graph nodes/edges
3. real `context/graph`
4. TUI graph browser
5. advertised connected-agent handoff flow
6. autonomous repo-context writes with provenance
