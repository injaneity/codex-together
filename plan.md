# Codex-Together v2 Plan

## 1. Summary

`codex-together` v2 is a graph-based context management system for multi-user collaboration.

The product is built around a simple model:

- users work in normal Codex threads
- threads can be visible to collaborators or hidden
- a focused `/context` view shows how threads, files, unresolved questions, and repo context relate
- users can act from that graph by attaching context, creating a handoff, or writing durable repo context files
- durable shared context lives as tracked Markdown under `.codex/context/`

The graph is not a separate app. It is a focused inspection and action surface for understanding and routing context across multiple users and multiple threads.

The UX split is intentional:

- normal chat, search, and command usage remain Codex-native
- only the `/context` graph view borrows the inspection and multi-select feel of LazyGit

## 2. Product Thesis

`codex-together` v2 is not primarily a thread-sharing product and not primarily a memory product.

It is a collaborative context-routing product.

It answers three questions:

1. What work is already happening in this repo?
2. What context from other threads or repo knowledge should inform this task?
3. What context from this thread should become durable shared repo knowledge?

## 3. Product Goals

### Goals

- Make collaboration multi-user by default, not single-user with optional sharing.
- Make relationships between threads, files, and durable repo context visible.
- Keep normal chat interaction feeling like Codex.
- Make shared durable context reviewable in Git.
- Make handoff cleaner than naive thread continuation or fork-heavy workflows.
- Prevent stale context through branch-aware resolution and explicit approval.
- Keep the user-facing command surface small.

### Non-Goals

- A free-roam general-purpose graph editor.
- A hidden memory system that silently rewrites shared durable knowledge.
- A large command taxonomy for graph maintenance.
- Replacing core Codex thread execution semantics.
- Making every kind of private scratch or note-taking a first-class concept in v1.

## 4. User Mental Model

There are only two user-visible context layers.

### 4.1 Local Thread Context

This is the live working context for a thread.

It includes:

- attached files
- related threads
- unresolved questions
- temporary findings
- handoff candidates
- selected repo context references

It is mutable and operational.

It may be:

- shared
- hidden

The default visibility is configurable in settings.

Default in v2:

- new threads are shared/public to connected collaborators by default

Here "public" means visible within the active repo collaboration session, not public on the internet.

### 4.2 Repo Context

This is durable shared knowledge stored as tracked files in the repository.

Default root:

- `.codex/context/`

Initial top-level folders:

- `concepts/`
- `hotspots/`
- `playbooks/`
- `decisions/`

Repo context is:

- branch-aware
- reviewable in Git
- explicit to write
- explicit to update
- never silently rewritten

### 4.3 Derived Context Graph

The graph is the operational model that connects:

- local thread context
- repo context files
- files and file clusters
- unresolved questions
- handoff edges

The graph exists for:

- search
- ranking
- routing
- inspection
- multi-select actions inside `/context`

The graph is not the primary persistence mechanism for durable shared context.

Durable shared context is the tracked Markdown file set.

## 5. Core Concepts

### Shared Thread

A thread that is visible to connected collaborators.

Collaborators can:

- inspect it
- attach context from it
- create a handoff from selected context in it

Collaborators cannot:

- write directly into another user's thread

### Inspect Mode

Inspect mode is a temporary read-only view of a selected shared thread.

- the transcript is visible
- the composer is disabled
- `/context` and `/handoff` still work
- `Esc` returns to the original thread

### Handoff

Handoff creates a fresh writable thread from a selected subgraph.

It is the main continuation primitive for collaborative work.

Handoff means:

- select the relevant context
- create a new thread
- carry forward only what the next thread needs

### Context Write

Context write is the act of turning selected graph material into tracked repo context files.

This is not exposed as a slash command.

Instead, it is an action inside `/context`, similar to how LazyGit lets users mark multiple commits before applying an action.

### Inline Context Attach

Typing `##` inside the composer opens a context attach palette.

Selecting a result inserts a structured context token into the draft instead of pasting raw text.

## 6. User-Facing Command Surface

| Command | Purpose | Notes |
| --- | --- | --- |
| `/host` | Start or manage the collaboration host for the current repo | Opens host sheet if already hosting |
| `/join <invite-or-url>` | Join a collaboration host | Enables shared threads and `/context` |
| `/leave` | Disconnect from the collaboration host | Exits inspect mode first if needed |
| `/share on` | Make the current thread visible to collaborators | Default may already be shared via settings |
| `/share off` | Hide the current thread from collaborators | Does not delete local thread state |
| `/threads [query]` | Browse and search shared threads | `Enter` inspects, `g` opens `/context` centered there |
| `/context [query]` | Open the graph inspection/action view | Main graph surface |
| `/handoff [goal]` | Create a fresh thread from selected current or inspected context | Can also be launched from `/context` |
| `##` | Inline context attach/search trigger | Main composer entry for context routing |

There is no standalone `/promote` command in v2.

Writing durable repo context happens inside `/context`.

## 7. Settings

The main v2 settings are:

- `collaboration.default_thread_visibility = public | private`
- `collaboration.repo_context_root = ".codex/context"`
- `collaboration.attach_branch_mismatch = ask | exclude`
- `collaboration.context_write_kind_defaults = [...]`

Recommended defaults:

- thread visibility: `public`
- repo context root: `.codex/context`
- branch mismatch: `ask`

## 8. UI Principles

### 8.1 Codex-Native By Default

Normal product surfaces follow Codex TUI conventions:

- transcript-first layout
- composer at the bottom
- bottom-pane pickers and sheets
- concise footer hints
- lightweight banners and status lines
- Codex-safe accent colors only

### 8.2 `/context` Is The Only LazyGit-Like Surface

`/context` should feel closer to LazyGit's commit graph and diff inspection model:

- left graph/list pane
- right detail pane
- lower preview pane
- terse action footer
- multi-select graph nodes
- action popups for handoff and context write

This applies only to the graph surface.

### 8.3 Accent Semantics

Use Codex-safe accents:

- `cyan` for selection and active state
- `green` for will-write / kept / added
- `red` for will-drop / removed
- `magenta` for Codex-generated previews and write summaries
- `dim` for secondary metadata and branch-mismatch state

There is no custom gold highlight in v2.

## 9. UI Sketches

### 9.1 Normal Thread View

```text
┌──────────────────────────────────────────────────────────────────────────────┐
│ Shared thread · branch main · 3 attached refs                               │
├──────────────────────────────────────────────────────────────────────────────┤
│ You                                                                         │
│ fix the mobile expiry issue using [ctx: token refresh invariant]            │
│                                                                              │
│ Codex                                                                       │
│ I found two related threads and one repo context file.                      │
│ The issue likely starts in session.ts after app resume.                     │
│                                                                              │
│ … transcript continues …                                                    │
├──────────────────────────────────────────────────────────────────────────────┤
│ >                                                                           │
├──────────────────────────────────────────────────────────────────────────────┤
│ Enter send  ## attach context  / commands  Esc cancel                       │
└──────────────────────────────────────────────────────────────────────────────┘
```

### 9.2 Inspect Mode Banner

```text
╔══ Inspecting: thr_188 · @alice · read-only · Esc to return ════════════════╗
║ attach context, open /context, or run /handoff from this thread            ║
╚══════════════════════════════════════════════════════════════════════════════╝
```

The composer is disabled while inspecting another user's thread.

### 9.3 `/threads` Picker

```text
┌ Shared Threads ─ search shared work ────────────────────────────────────────┐
│ > thr_291  mobile refresh expiry        @you     12m ago   shared          │
│   thr_188  auth regression              @alice   2h ago    shared          │
│   thr_322  deploy follow-up             @bob     1d ago    shared          │
│                                                                              │
│ Preview: mobile expiry after app background/resume                           │
│                                                                              │
│ Enter inspect  g context  / search  Esc close                                │
└──────────────────────────────────────────────────────────────────────────────┘
```

### 9.4 `##` Attach Palette

```text
┌ Attach Context ─ ## token refresh ──────────────────────────────────────────┐
│ > concept   token-refresh-invariant          repo context                   │
│   thread    thr_188 auth regression          @alice                         │
│   file      server/auth/session.ts           touched in 4 threads           │
│   issue     mobile expiry mismatch           unresolved                     │
│                                                                              │
│ Enter attach  Space mark  Tab preview  Esc cancel                            │
└──────────────────────────────────────────────────────────────────────────────┘
```

### 9.5 `/context` Graph View

This is the only view that should feel LazyGit-like.

```text
┌ Context ───────────────────────────────────┬ Node ───────────────────────────┐
│                                            │ (T) thr_291 mobile expiry       │
│ ● thr_188 auth regression @alice           │ owner: you                      │
│ │                                          │ branch: main                    │
│ ├● thr_291 mobile refresh expiry           │ visibility: shared              │
│ │├─<> server/auth/session.ts               │                                 │
│ │├─[M] token-refresh-invariant             │ Summary                         │
│ │└─[?] why early expiry after resume?      │ mobile session expires early    │
│ │                                          │ after backgrounding app         │
│ └● thr_322 deploy follow-up @bob           │                                 │
│   └─[M] auth-deploy-checklist              │ Refs                            │
│                                            │ 2 files · 1 memory · 1 issue    │
├ Filters ───────────────────────────────────┼ Preview ────────────────────────┤
│ scope: shared + local                      │ Related                         │
│ type: all                                  │ <> tests/auth_spec.ts           │
│ branch: main                               │ [M] mobile-session-expiry       │
│                                            │                                 │
│ / filter                                   │ Recent handoff                  │
│ s shared only                              │ thr_188 -> thr_291              │
│ t threads only                             │                                 │
├────────────────────────────────────────────┴─────────────────────────────────┤
│ ↑↓ move  Space mark  Enter open  a attach  h handoff  w write  / filter q  │
└──────────────────────────────────────────────────────────────────────────────┘
```

### 9.6 `/context` Graph View While Preparing A Write

Marked nodes are shown as selected. The resulting write set is previewed with Codex-safe accents:

- `cyan` selected nodes
- `green` nodes that will be written into repo context
- `red` nodes excluded from the write set
- `magenta` generated write preview text

```text
┌ Context ───────────────────────────────────┬ Write Preview ───────────────────┐
│ ● thr_188 auth regression @alice           │ will write: 2 repo context files │
│ │                                          │                                  │
│ ├● thr_291 mobile refresh expiry           │ + concepts/token-refresh-        │
│ │├─<> server/auth/session.ts               │   invariant.md                   │
│ │├─[M] token-refresh-invariant             │ + hotspots/mobile-session-       │
│ │└─[?] why early expiry after resume?      │   expiry.md                      │
│ │                                          │                                  │
│ └● thr_322 deploy follow-up @bob           │ source threads: thr_188, thr_291 │
│                                            │ branch: main                     │
├ Filters ───────────────────────────────────┼ Details ─────────────────────────┤
│ marked: 4                                  │ green = write                    │
│ branch check: clean                        │ red = exclude                    │
│                                            │ cyan = selected                  │
├────────────────────────────────────────────┴──────────────────────────────────┤
│ Space mark  w review write  h handoff  Esc clear marks  q close              │
└───────────────────────────────────────────────────────────────────────────────┘
```

### 9.7 Context Write Review Popup

This is launched from `/context` after the user marks nodes and selects write.

```text
┌ Write Repo Context ──────────────────────────────────────────────────────────┐
│ Source selection: 4 graph nodes                                              │
│                                                                                │
│ Create                                                                        │
│ [x] .codex/context/concepts/token-refresh-invariant.md                        │
│ [x] .codex/context/hotspots/mobile-session-expiry.md                          │
│                                                                                │
│ Evidence                                                                       │
│ [x] thr_188 auth regression                                                    │
│ [x] thr_291 mobile refresh expiry                                              │
│ [x] server/auth/session.ts                                                     │
│ [x] tests/auth_spec.ts                                                         │
│                                                                                │
│ Result: tracked Markdown files will be written as normal working tree edits    │
│                                                                                │
│ Enter write  Space toggle  Tab preview file  Esc cancel                        │
└───────────────────────────────────────────────────────────────────────────────┘
```

### 9.8 `/handoff` Popup

```text
┌ Handoff ─ create fresh thread from selected context ─────────────────────────┐
│ Goal: isolate mobile expiry investigation                                     │
│                                                                                │
│ Include                                                                        │
│ [x] thr_291 mobile refresh expiry                                              │
│ [x] server/auth/session.ts                                                     │
│ [x] token-refresh-invariant                                                    │
│ [x] why early expiry after resume?                                             │
│ [ ] thr_322 deploy follow-up                                                   │
│                                                                                │
│ Result: 1 thread root · 2 files · 1 memory item · 1 open question             │
│                                                                                │
│ Enter create  Space toggle  e edit goal  g open context  Esc cancel           │
└───────────────────────────────────────────────────────────────────────────────┘
```

## 10. Main Workflows

### 10.1 Start Collaboration

1. User runs `/host`.
2. Host starts for the current repo and shows invite information.
3. Other users run `/join <invite-or-url>`.
4. Shared threads become visible in `/threads`.
5. `/context` becomes meaningful across users, not just local data.

### 10.2 Work In A Normal Thread

1. User chats in the normal Codex thread view.
2. User types `##` to attach context from:
   - related threads
   - repo context files
   - files and file clusters
   - unresolved questions
3. Selected references become structured context refs in the draft.
4. On send, those refs are resolved into a branch-aware working bundle.

### 10.3 Inspect Another User's Thread

1. User runs `/threads`.
2. User selects a thread and presses `Enter`.
3. Thread opens in inspect mode.
4. User can:
   - read it
   - use `##` to attach from it
   - open `/context` centered on it
   - run `/handoff`
5. User cannot send messages into that thread.

### 10.4 Open `/context`

1. User runs `/context` with no query to center on current thread.
2. Or runs `/context auth` to search and center on a thread, file, or repo context file.
3. User moves selection through the graph.
4. User may:
   - attach selected nodes
   - mark multiple nodes
   - create a handoff from marked nodes
   - write repo context from marked nodes

### 10.5 Write Durable Repo Context

1. User opens `/context`.
2. User marks relevant nodes with `Space`.
3. User presses `w`.
4. Codex proposes one or more repo context files and previews their contents.
5. User explicitly approves.
6. Files are written under `.codex/context/` as normal tracked working tree edits.

### 10.6 Handoff

1. User launches `/handoff` from normal thread view or presses `h` inside `/context`.
2. The selected or current subgraph is compacted into a focused bundle.
3. User reviews the bundle.
4. On confirm, a fresh writable thread is created.
5. The original thread remains unchanged.

## 11. Exact Command Behavior

### 11.1 `/host`

Behavior:

- start collaboration hosting for the current repo
- if already hosting, open the host sheet
- if already joined elsewhere, require `/leave` first

Host sheet shows:

- invite URL
- member count
- current repo
- graph/index health
- stop hosting action

### 11.2 `/join <invite-or-url>`

Behavior:

- join a collaboration host
- fetch repo/session metadata
- enable shared thread discovery and `/context`

### 11.3 `/leave`

Behavior:

- disconnect from host
- exit inspect mode first if active
- keep local threads intact

### 11.4 `/share on`

Behavior:

- make current thread visible to collaborators
- include thread in `/threads`
- allow `/context` to connect it into the shared graph

### 11.5 `/share off`

Behavior:

- hide current thread from collaborators
- preserve local thread state
- keep existing repo context files unchanged

### 11.6 `/threads [query]`

Behavior:

- open searchable shared thread picker
- `Enter` opens inspect mode
- `g` opens `/context` centered on selected thread

### 11.7 `/context [query]`

Behavior:

- open the graph inspection/action surface
- if no query, center on current thread
- if query supplied, search and center on the best match
- allow multi-select and graph actions

Actions inside `/context`:

- `Up` / `Down`: move selection
- `Enter`: open selected node
- `Space`: mark / unmark node
- `a`: attach selected or marked nodes to current draft
- `h`: start handoff from selected or marked nodes
- `w`: review and write repo context from selected or marked nodes
- `/`: filter
- `q` or `Esc`: close

### 11.8 `/handoff [goal]`

Behavior:

- create a fresh writable thread from the current relevant subgraph
- if launched in inspect mode, use inspected thread as source
- if launched in `/context`, use selected or marked nodes

### 11.9 `##`

Behavior:

- inline trigger inside composer
- opens attach/search palette
- inserts structured context refs into draft

## 12. Send-Time Context Resolution

When the user submits a prompt:

1. Extract `ContextRef` tokens inserted via `##`.
2. Resolve them against the derived graph and current branch.
3. Build a working context bundle.
4. Exclude branch-mismatched or stale refs by default, or ask based on settings.
5. Inject the resolved bundle into the outgoing turn.
6. Start the turn through Codex app-server.

The attach system should prefer structured refs over raw pasted context.

## 13. Branch Awareness And Staleness Rules

Branch-awareness is mandatory in v2.

### Thread Metadata Used For Routing

Each thread should carry, when available:

- `repo_root`
- `git_branch`
- `git_sha`
- `git_origin_url`

### Repo Context File Metadata

Each repo context file should carry machine-readable front matter.

Example:

```yaml
---
id: token-refresh-invariant
kind: concept
title: Token Refresh Invariant
applies_to:
  branches:
    - main
source_threads:
  - thr_188
  - thr_291
source_files:
  - server/auth/session.ts
  - tests/auth_spec.ts
last_validated_at: 2026-03-13
visibility: repo
---
```

### Resolution Rules

- prefer exact branch matches first
- show branch mismatches as dim/stale in `/context` and `##` results
- exclude mismatches by default from send-time bundles
- allow explicit user inclusion if needed
- never present stale or branch-mismatched context as silently safe

## 14. Repo Context File Layout

Initial layout:

- `.codex/context/concepts/`
- `.codex/context/hotspots/`
- `.codex/context/playbooks/`
- `.codex/context/decisions/`

Recommended file purposes:

- `concepts/`: invariants, explanations, recurring domain truths
- `hotspots/`: tricky codepaths, known failure areas, sharp edges
- `playbooks/`: procedures and debugging workflows
- `decisions/`: durable team choices and tradeoffs

## 15. Graph Model

### 15.1 Node Types

MVP nodes:

- `Thread`
- `RepoContextFile`
- `FileCluster`
- `OpenQuestion`
- `Handoff`

People are labels on threads, not first-class graph nodes in v1.

### 15.2 Edge Types

MVP edges:

- `touches`
- `cites`
- `answers`
- `overlaps`
- `handoff_from`
- `derived_into`

### 15.3 Why This Graph Exists

The graph exists to answer:

- which other threads touch the same files or concepts?
- which durable context should inform this task?
- what subset should be handed off?
- what subset is stable enough to write into repo context files?

## 16. Persistence Model

### 16.1 Operational State

Operational collaboration state is service-backed and mutable:

- thread visibility
- inspect sessions
- live overlays
- graph marks and selections
- temporary unresolved questions
- ranking/search indexes

### 16.2 Durable Shared State

Durable shared state is the tracked repo context file set under `.codex/context/`.

This is the review surface for shared knowledge.

### 16.3 Derived Index

The collaboration service builds a derived graph/index from:

- live thread state
- visible shared threads
- tracked repo context files
- file/path relationships
- handoff lineage

The index is rebuildable.

## 17. Trust And Governance

### 17.1 Thread Permissions

- thread owner can write to their thread
- collaborators can inspect shared threads
- collaborators can attach context from shared threads
- collaborators can create handoffs from shared threads
- collaborators cannot directly mutate another user's thread

### 17.2 Repo Context Write Rules

- writing repo context always requires explicit approval
- bulk write approval is allowed
- no silent write or rewrite of tracked files
- resulting changes appear as normal tracked working tree edits

### 17.3 Why Git-Tracked Files Matter

Using tracked files for durable shared context gives:

- reviewable diffs
- branch semantics
- merge behavior
- rollback
- familiar GitHub/GitLab workflows

## 18. Internal Service Responsibilities

### 18.1 Execution Plane

Codex CLI / app-server remains responsible for:

- thread lifecycle
- turn lifecycle
- transcript execution
- real writable threads

### 18.2 Collaboration Plane

The collaboration service owns:

- host/join/leave lifecycle
- thread visibility
- shared thread discovery
- inspect bundles
- graph building
- context search and ranking
- branch-aware context resolution
- handoff planning
- repo context write planning

## 19. Internal RPC Shape

Representative internal RPCs:

- `host/start(repo_root) -> HostSession`
- `host/status() -> HostStatus`
- `session/join(invite_or_url) -> RepoSession`
- `session/leave() -> {}`
- `thread/share(thread_id, visible) -> {}`
- `thread/list(query?) -> [ThreadRow]`
- `thread/inspect(thread_id) -> ThreadInspectionBundle`
- `context/search(query, scope) -> [ContextResult]`
- `context/graph(center, filters) -> ContextGraph`
- `context/preview(node_id) -> NodePreview`
- `context/resolveBundle(thread_id, context_refs, branch) -> WorkingContextBundle`
- `handoff/plan(source, selection, goal?) -> HandoffPlan`
- `handoff/commit(plan_id) -> NewThreadInfo`
- `context/writePlan(selection, branch) -> ContextWritePlan`
- `context/writeCommit(plan_id) -> WrittenFiles`

There is no user-facing `/promote` command even though context writing is a first-class internal action.

## 20. Data Structures

### 20.1 ThreadRow

- `thread_id`
- `owner`
- `title`
- `summary`
- `visibility`
- `updated_at`
- `git_branch`

### 20.2 ThreadInspectionBundle

- `thread_id`
- `owner`
- `summary`
- `recent_excerpt`
- `attached_refs`
- `related_repo_context`
- `graph_center`

### 20.3 ContextRef

- `ref_id`
- `kind`
- `display_label`
- `source_thread_id` or `repo_context_id`
- `git_branch`
- `stale_state`

### 20.4 ContextGraph

- `nodes`
- `edges`
- `center_node_id`
- `filters`
- `branch`

### 20.5 HandoffPlan

- `plan_id`
- `source_thread_id`
- `goal`
- `selected_node_ids`
- `kept_refs`
- `dropped_refs`
- `token_estimate`

### 20.6 ContextWritePlan

- `plan_id`
- `selected_node_ids`
- `target_files`
- `generated_previews`
- `source_threads`
- `source_files`
- `branch`

## 21. Key Design Decisions

### Decision: No Standalone `/promote`

Why:

- durable context writing is graph-selection work
- it should feel like acting on marked graph nodes, not invoking a separate memory command
- `/context` is the correct surface for inspect + select + review + write

### Decision: `/context` Replaces `/map`

Why:

- "context" communicates the user goal better than "map"
- the view is both an inspection tool and an action surface

### Decision: `##` Replaces `??`

Why:

- it reads more naturally as an inline context trigger
- it is easier to explain as "type `##` to attach context"

### Decision: Public By Default, Configurable

Why:

- the product is collaborative by default
- advanced users can still choose private-by-default in settings

### Decision: Durable Shared Context Is Git-Tracked

Why:

- users need reviewability and trust
- branch-awareness is native in Git
- durable shared context should live beside code, not only inside a service

### Decision: Codex-Native Everywhere Except `/context`

Why:

- the product should still feel like Codex
- the graph view benefits from LazyGit-like inspection affordances
- copying LazyGit's entire UX would make the product feel disjointed

## 22. Failure Handling

- If the collaboration service is unavailable, normal local Codex still works.
- If a shared thread disappears during inspect mode, show stale state and require `Esc` back.
- If branch-aware resolution cannot safely attach a ref, dim it and exclude it by default.
- If handoff planning fails, do not mutate the source thread.
- If repo context write planning fails, do not write any files.
- If a write is approved but file writes fail, surface the exact file errors and keep the plan available for retry.

## 23. MVP Delivery Plan

### Phase 1

- host/join/leave
- thread visibility with `/share on|off`
- `/threads`
- inspect mode for shared threads

### Phase 2

- `##` inline attach
- context search across threads, files, and repo context
- send-time bundle resolution

### Phase 3

- `/context`
- graph inspection
- multi-select actions
- graph-centered attach flow

### Phase 4

- `/handoff` from normal threads and `/context`
- branch-aware bundle compaction
- handoff previews

### Phase 5

- repo context write flow from `/context`
- tracked Markdown file generation under `.codex/context/`
- explicit approval and file previews

### Phase 6

- ranking improvements
- richer graph previews
- better stale-context diagnostics

## 24. Engineering Integration Points

Likely touchpoints in the current codebase:

- TUI composer and slash-command handling
- bottom-pane list/picker/sheet infrastructure
- inspect-mode thread switching and read-only handling
- app-server thread read/start lifecycle integration
- git metadata capture for branch-aware routing
- new collaboration service for graph/index resolution
- tracked repo-context file generation under `.codex/context/`

## 25. Open Questions

These are still intentionally open after the v2 spec:

- should repo context writes prefer creating many small files or updating fewer broader files?
- should branch-specific repo context files merge upward automatically through normal Git workflows only, or should the service offer branch-diff guidance?
- should shared threads support per-thread labels or status states in v1, or stay query-and-graph only?
- should `/context` support left/right pane focus switching explicitly, or remain single-selection with fixed pane responsibilities?

## 26. Final Product Principle

`Shared by visibility, routed by graph, durable by review.`

## 27. External References

Official references that informed the v2 direction:

- Amp Handoff: <https://ampcode.com/news/handoff>
- Amp Read Threads: <https://ampcode.com/news/read-threads>
- Amp Find Threads: <https://ampcode.com/news/find-threads>
- Amp Thread Map: <https://ampcode.com/news/thread-map>
- Amp fork removal / handoff emphasis: <https://ampcode.com/news/stick-a-fork-in-it>
- GitHub Copilot Memory: <https://docs.github.com/en/copilot/concepts/agents/copilot-memory>
- GitHub Copilot Spaces: <https://docs.github.com/en/copilot/concepts/context/spaces>
