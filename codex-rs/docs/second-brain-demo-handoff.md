# Second Brain Demo Handoff

Last updated: 2026-03-18

## Goal

Build a Codex Together demo where a repo acts as a "second brain" for humans and OpenClaw-style agents working through Codex threads.

For the current proposed graph shape, see `docs/context-graph-model.md`.

Core idea:

- repo is the canonical durable memory
- Codex threads are live working memory
- a context graph is a rebuildable index over repo knowledge plus thread/agent activity
- handoff is the primary coordination flow
- agents are autonomous and visible, not subordinate-only tools

## Product decisions already made

These are the decisions the next thread should treat as settled unless the user explicitly changes them.

### Demo flow

1. User starts a thread and asks Codex to do light research.
2. That thread contributes to a context graph for the repo.
3. User can `/handoff` to connected agents that the graph advertises as relevant.
4. User can also `/handoff` to self, which creates another Codex thread under the same human actor.
5. Agents can hand off to other agents, reuse repo context, and update repo knowledge autonomously.
6. User can inspect repo-wide context hotspots and reused knowledge at a glance.
7. From the agent perspective, connecting to the repo/server should immediately expose the full persisted repo context.

### UX / behavior

- Graph should live in the existing TUI, in a LazyGit-like visual style.
- OpenClaw agents should be represented in the UI with the lobster emoji.
- Threads are shared by default.
- Active agents are advertised by default.
- Only currently connected agents should be advertised for now.
- Agents should have broad autonomy, including real git commits.
- Explicit exception: agents should not be allowed to delete the repo or remove people.
- Friction should be minimized aggressively.
- If a feature is not needed for the demo flow, it should be removed from the Together product surface.
- Existing Codex app-server methods can remain even if the demo does not directly use all of them.

### Architecture direction

- Repo-backed notes are the canonical second brain.
- `.codex/context/` is still the intended long-term canonical location.
- Thread-local context should keep extracted artifacts, not raw prompts or raw search queries.
- Thread-local context should be reconstructed from native Codex thread history and exposed through the shared graph engine; Together should consume that view, not own a separate thread-memory source of truth.
- Attached context should use progressive disclosure: give the model refs, summaries, hotspots, and traversal paths first, not full concatenated bodies.
- `context_graph` should be a native core tool backed by the same shared graph engine that powers `/context`.
- The graph model should use one synthetic `anchor` node and two real content node types: `thread` and `repo`.
- Thread context should remain one rooted graph projection, not separate "self-thread" and "lineage" storage systems.
- Fork and handoff provenance should be represented through anchor metadata plus mounted/provenance edges.
- Runtime graph indexes can live under `.codex/context/.graph/` as ignored, rebuildable discovery artifacts.
- Promoted notes should keep direct source graph refs so persistent memory stays tied to the artifact graph instead of only whole-thread backlinks.
- `codex-together` should be pruned back toward native Codex CLI behavior before adding graph/handoff demo features.
- `codex-together-server` should become the collaboration and discovery boundary for both Codex and non-Codex agents.

## What was discussed before pruning

The high-level implementation plan that was agreed before the graph simplification:

- First-class actor model for humans and agents
- richer graph node taxonomy including actor/thread/repo-note/file/handoff
- Real graph edges instead of search-only stubs
- Durable server-side handoff objects
- Repo-backed context writes with provenance
- Autonomous agent writes and commits
- TUI `/context` as the main browsing surface
- TUI `/handoff` as the main routing surface

Current direction:

- keep the actor model and handoff flow
- simplify the graph to:
  - one synthetic anchor
  - thread nodes
  - repo nodes
- express fork/handoff/imported context through metadata and edges instead of more top-level node kinds

Important long-term design calls:

- repo = source of truth
- graph = rebuildable index
- app-server = execution substrate
- together = collaboration UI / context routing layer

## Pruning principles agreed with the user

- remove or de-emphasize hosted thread-sharing behavior that deviates from native Codex CLI
- remove thread visibility concepts from the demo
- remove unnecessary ACL/member-management UX from the demo surface
- avoid extra collaboration metaphors
- keep host/join/leave as thin transport/admin commands only
- do not rebase first; prune first

## Current implementation status

Pruning has started and a large amount of old Together surface has already been removed. See `docs/second-brain-prune-status.md` for the exact code-level status.

At a high level, the current branch has already been pushed much closer to the desired demo model:

- `/share` and `/threads` were removed from the TUI slash-command surface
- read-only "inspect/checkout shared thread" flows were removed
- thread visibility concepts were removed from repo-context metadata and related UI copy
- `/context` now roots on the current thread and keeps retained artifacts plus linked repo notes
- context search now indexes thread-local artifacts and persistent repo notes instead of prompt previews
- attached context bundles now prefer compact discovery packs over eager full-body expansion
- runtime graph artifacts are intended to be discoverable under `.codex/context/.graph/`
- repo note promotion should preserve direct artifact refs for graph-derived provenance
- persisted member bookkeeping was pruned from the state layer
- unused member-update notifications were removed
- user-facing Together status copy was simplified toward "server + participants" instead of "owner + member"

## Expected next implementation phase after prune baseline is stable

Once the next thread confirms the prune baseline is green, the next major workstream should be:

1. Unified anchor/thread/repo graph schema
2. Rooted graph queries and provenance-aware traversal
3. TUI `/context` graph view with "here" / "from prev" / "handoff" / "repo" treatments
4. TUI `/handoff` addressed-target flow for connected agents
5. Shared thread ingestion and discovery for non-Codex agents
6. Additive repo-memory promotion from selected thread nodes

## Important warning for the next thread

The repo-local handoff notes were saved under `codex-rs/docs/` because this session only had write access inside `codex-rs/`.

If the next thread has write access at the repo root, it may make sense to later mirror or move this into the canonical repo memory location under `.codex/context/`.
