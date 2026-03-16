# `codex-2gether`

`codex-2gether` is the v2 rewrite of Codex Together.

The rewrite keeps one seam from v1: the Codex app-server bridge for thread lifecycle and thread reads. Everything above that seam is collaboration-specific v2 code.

## Current User Surface

Primary commands:

- `/host`
- `/join <invite-or-url>`
- `/leave`
- `/share on|off`
- `/threads`
- `/context [query]`
- `/handoff [goal]`
- `##` inline context attach inside the composer

Removed v1 commands:

- `/together`
- `/history`
- `/close`
- shared-thread fork-first workflows
- Together Center / mascot / presence UI

## V2 Behavior

### Hosting and membership

- `/host` starts or reconnects the local collaboration host.
- `/join` connects to another host by invite or URL.
- `/leave` disconnects without mutating local threads.

### Shared threads

- `/share on|off` toggles visibility for the current thread.
- `/threads` is inspect-only.
- Inspect mode replays another shared thread read-only and returns to the prior local thread on `Esc`.

### Context

- `/context` searches shared threads and tracked repo context files under `.codex/context/`.
- `Enter` attaches the current context item into the composer.
- `Space` marks context items for multi-item actions.
- `Shift+H` creates a handoff plan from the current or marked items.
- `Shift+W` opens a tracked-file write review for the current or marked items.
- `##query` opens inline attach search from the composer and inserts bound `[ctx: ...]` tokens.
- On submit, bound context is resolved with `context/resolveBundle` and prepended as a hidden collaboration bundle.

### Handoff

- `/handoff` plans a fresh-thread handoff from the current thread.
- Handoffs commit through `thread/start`.
- The new thread opens writable with a prefilled draft and bound context refs.

### Durable repo context

- Repo context is stored as tracked Markdown under `.codex/context/`.
- `/context` write review calls `context/writePlan` and `context/writeCommit`.
- Existing repo context files are only updated when that exact repo-context node is selected.
- New durable notes are written as normal working-tree edits.

## Wire Protocol

The v2 collaboration RPC surface is:

- `host/start`
- `host/status`
- `host/stop`
- `session/join`
- `session/leave`
- `thread/share`
- `thread/list`
- `thread/inspect`
- `context/search`
- `context/graph`
- `context/preview`
- `context/resolveBundle`
- `context/writePlan`
- `context/writeCommit`
- `handoff/plan`
- `handoff/commit`

The old v1 collaboration RPC aliases are removed from the active server/TUI flow. The remaining `together/auth` bootstrap call is an internal compatibility seam, not part of the intended user-facing surface.

## Durable Context Format

Tracked repo context files live under `.codex/context/` and use front matter like:

```yaml
---
id: "token-refresh-invariant"
kind: "concept"
title: "Token Refresh Invariant"
applies_to:
  branches:
    - "main"
source_threads:
  - "thr_188"
source_files:
  - "server/auth/session.ts"
last_validated_at: 2026-03-16
visibility: "repo"
---
```

Initial directories:

- `.codex/context/concepts/`
- `.codex/context/hotspots/`
- `.codex/context/playbooks/`
- `.codex/context/decisions/`

## Architecture

The control flow is:

1. The TUI dispatches a collaboration command or inline context action.
2. `codex-together-protocol` carries JSON-RPC over the Together websocket.
3. `codex-together-server` owns host/join/share/search/handoff/write orchestration.
4. The server uses the existing Codex app-server bridge for `thread/read`, `thread/start`, and resume-related metadata.
5. Durable repo context is read and written from `.codex/context/`.

## Current Gaps

The remaining rewrite work is:

- replace the current `/context` picker with the planned graph-native UI
- finish broader docs/config cleanup around the new collaboration model
