# Context Graph Model

Last updated: 2026-03-18

Status: proposed design

## Goal

Define a single context graph model that:

- works for Codex CLI and non-Codex agents
- keeps local thread context and repo-wide knowledge in one rooted graph
- makes fork and handoff provenance visible
- supports additive promotion into `.codex/context/...`
- keeps the server as the collaboration and discovery boundary

This document is intentionally narrower than the earlier "second brain" notes. It focuses on the graph shape, traversal semantics, and the agent-facing data model.

## Summary

The proposed model has:

- one logical context graph per repo/workspace
- one synthetic `anchor` node per query/viewpoint
- two real content node classes:
  - `thread`
  - `repo`

The graph is not meant to be dumped in full into prompts. Agents query rooted projections from the current anchor and expand outward as needed.

## Design Principles

1. The repo owns durable memory.
2. Threads own working memory.
3. The graph is the discovery layer over both.
4. Fork and handoff should be visible as provenance, not as separate memory systems.
5. Promotion is additive by default.
6. The Codex CLI is one client, not the only client.

## Node Types

### `anchor`

The `anchor` is a synthetic root for the current query context. It is not durable memory. It describes the current viewpoint.

Suggested fields:

```json
{
  "nodeType": "anchor",
  "anchorId": "anchor:thread-2",
  "currentThreadId": "thread-2",
  "precursorThreadId": "thread-1",
  "precursorKind": "handoff",
  "actorId": "reviewer@local",
  "repoRoot": "/repo",
  "gitBranch": "rewrite-codex-2gether-v2",
  "goal": "Verify the simplified /context and /handoff flow."
}
```

Notes:

- `precursorThreadId` is optional.
- `precursorKind` is optional and should be `fork` or `handoff` when present.
- The server can construct different anchors for different queries without changing the underlying memory model.

### `thread`

`thread` nodes are thread-specific context artifacts. This is the agent's working memory layer.

Examples:

- retained plan output
- file reads and file changes
- search results
- tool output
- context graph queries
- imported handoff or fork seed nodes

Suggested fields:

```json
{
  "nodeType": "thread",
  "nodeId": "ctx:thread-insight:thread-1:plan-2",
  "artifactKind": "plan",
  "title": "Simplify /context selection flow",
  "summary": "thread insight · retained plan output",
  "location": "insight/plan-2",
  "body": "Only show one-line nodes and let Enter toggle selection.",
  "originThreadId": "thread-1",
  "sourceFiles": [],
  "sourceRefs": [],
  "createdAt": 1773792000
}
```

Notes:

- `originThreadId` is required. It answers "which thread created this?"
- `artifactKind` is metadata, not a top-level node class. This avoids adding many node enums while preserving detail.
- Imported handoff and fork nodes remain `thread` nodes. Their origin does not change.

### `repo`

`repo` nodes are repo-wide durable memory stored under `.codex/context/...`.

Examples:

- concepts
- decisions
- playbooks
- hotspots

Suggested fields:

```json
{
  "nodeType": "repo",
  "nodeId": "ctx:file:.codex/context/playbooks/handoff-selection-flow.md",
  "repoKind": "playbook",
  "title": "Handoff selection flow",
  "summary": "Selection-only handoff UI with auto-promotion on commit.",
  "path": ".codex/context/playbooks/handoff-selection-flow.md",
  "sourceThreads": ["thread-1"],
  "sourceRefs": ["ctx:thread-insight:thread-1:plan-2"],
  "sourceFiles": ["tui/src/chatwidget.rs"],
  "lastValidatedAt": "2026-03-18"
}
```

Notes:

- Repo nodes are the shared durable memory layer.
- Coverage and promotion work by linking repo nodes back to thread nodes via `sourceRefs`, `sourceThreads`, and `sourceFiles`.

## Edge Types

The model should keep the edge vocabulary small and make provenance explicit.

### `mounted`

Used from the current `anchor` to nodes that are in the current working set.

Examples:

- `anchor -> thread` with `mountReason = "local"`
- `anchor -> thread` with `mountReason = "handoff_seed"`
- `anchor -> thread` with `mountReason = "fork_seed"`
- `anchor -> repo` with `mountReason = "repo_neighbor"`

Suggested shape:

```json
{
  "from": "anchor:thread-2",
  "to": "ctx:thread-insight:thread-1:plan-2",
  "edgeType": "mounted",
  "mountReason": "handoff_seed"
}
```

### `related`

Used for relevance links between content nodes.

Examples:

- same source file
- same source ref
- same origin thread
- query/neighbor relationships produced by graph traversal

Suggested shape:

```json
{
  "from": "ctx:thread-file:thread-2:tui-src-chatwidget-rs",
  "to": "ctx:thread-insight:thread-1:plan-2",
  "edgeType": "related",
  "reason": "same_file"
}
```

### `covered_by`

Used when a repo note already covers a thread node.

Suggested shape:

```json
{
  "from": "ctx:thread-insight:thread-1:plan-2",
  "to": "ctx:file:.codex/context/playbooks/handoff-selection-flow.md",
  "edgeType": "covered_by"
}
```

### `promoted_to`

Used when a thread node directly led to a repo note.

Suggested shape:

```json
{
  "from": "ctx:thread-insight:thread-2:plan-1",
  "to": "ctx:file:.codex/context/hotspots/selection-footguns.md",
  "edgeType": "promoted_to"
}
```

## Provenance Rules

The main provenance questions are:

- Which thread created this node?
- Why is this node visible in the current thread?
- Is this node already persisted in repo memory?

The model answers those questions with:

- node metadata
  - `originThreadId`
  - `artifactKind`
  - `sourceThreads`
  - `sourceRefs`
- anchor metadata
  - `currentThreadId`
  - `precursorThreadId`
  - `precursorKind`
- edge metadata
  - `mountReason`
  - `reason`

This keeps the model simple while still supporting the UI treatments we discussed:

- `here`
  `originThreadId == currentThreadId` and `mountReason = local`
- `from prev`
  `originThreadId == precursorThreadId`
- `handoff`
  `mountReason = handoff_seed`
- `fork`
  `mountReason = fork_seed`
- `repo`
  `nodeType = repo`

## Query Model

The server should expose rooted graph queries instead of forcing clients to reconstruct the graph themselves.

Suggested RPC surface:

- `thread/start`
- `thread/appendItems`
- `thread/read`
- `thread/list`
- `context/query`
- `context/open`
- `context/hotspots`
- `memory/promote`

### `context/query`

This should return a rooted graph projection from the current anchor.

Suggested request:

```json
{
  "currentThreadId": "thread-2",
  "precursorThreadId": "thread-1",
  "precursorKind": "handoff",
  "query": null,
  "seedRefIds": [
    "ctx:thread-insight:thread-1:plan-2"
  ],
  "limit": 12
}
```

Suggested response:

```json
{
  "anchor": {
    "nodeType": "anchor",
    "anchorId": "anchor:thread-2",
    "currentThreadId": "thread-2",
    "precursorThreadId": "thread-1",
    "precursorKind": "handoff",
    "actorId": "reviewer@local",
    "repoRoot": "/repo",
    "gitBranch": "rewrite-codex-2gether-v2",
    "goal": "Verify the simplified /context and /handoff flow."
  },
  "nodes": [
    {
      "nodeType": "thread",
      "nodeId": "ctx:thread-insight:thread-1:plan-2",
      "artifactKind": "plan",
      "title": "Simplify /context selection flow",
      "summary": "thread insight · retained plan output",
      "location": "insight/plan-2",
      "body": "Only show one-line nodes and let Enter toggle selection.",
      "originThreadId": "thread-1",
      "sourceFiles": [],
      "sourceRefs": [],
      "createdAt": 1773792000
    },
    {
      "nodeType": "thread",
      "nodeId": "ctx:thread-file:thread-2:tui-src-chatwidget-rs",
      "artifactKind": "file_change",
      "title": "tui/src/chatwidget.rs",
      "summary": "linked file · updated in thread",
      "location": "tui/src/chatwidget.rs",
      "body": "Adjusted context empty-state copy.",
      "originThreadId": "thread-2",
      "sourceFiles": ["tui/src/chatwidget.rs"],
      "sourceRefs": [],
      "createdAt": 1773792200
    },
    {
      "nodeType": "repo",
      "nodeId": "ctx:file:.codex/context/playbooks/handoff-selection-flow.md",
      "repoKind": "playbook",
      "title": "Handoff selection flow",
      "summary": "Selection-only handoff UI with auto-promotion on commit.",
      "path": ".codex/context/playbooks/handoff-selection-flow.md",
      "sourceThreads": ["thread-1"],
      "sourceRefs": ["ctx:thread-insight:thread-1:plan-2"],
      "sourceFiles": ["tui/src/chatwidget.rs"],
      "lastValidatedAt": "2026-03-18"
    }
  ],
  "edges": [
    {
      "from": "anchor:thread-2",
      "to": "ctx:thread-insight:thread-1:plan-2",
      "edgeType": "mounted",
      "mountReason": "handoff_seed"
    },
    {
      "from": "anchor:thread-2",
      "to": "ctx:thread-file:thread-2:tui-src-chatwidget-rs",
      "edgeType": "mounted",
      "mountReason": "local"
    },
    {
      "from": "anchor:thread-2",
      "to": "ctx:file:.codex/context/playbooks/handoff-selection-flow.md",
      "edgeType": "mounted",
      "mountReason": "repo_neighbor"
    },
    {
      "from": "ctx:thread-insight:thread-1:plan-2",
      "to": "ctx:file:.codex/context/playbooks/handoff-selection-flow.md",
      "edgeType": "covered_by"
    },
    {
      "from": "ctx:thread-file:thread-2:tui-src-chatwidget-rs",
      "to": "ctx:thread-insight:thread-1:plan-2",
      "edgeType": "related",
      "reason": "same_file"
    }
  ]
}
```

## Sample Graph

This is the same example as a compact visual graph.

```text
anchor: thread-2
  currentThreadId: thread-2
  precursorThreadId: thread-1
  precursorKind: handoff
  actorId: reviewer@local

  ├─ mounted(local)
  │    -> [thread] tui/src/chatwidget.rs
  │       originThreadId: thread-2
  │       artifactKind: file_change
  │
  ├─ mounted(handoff_seed)
  │    -> [thread] Simplify /context selection flow
  │       originThreadId: thread-1
  │       artifactKind: plan
  │
  └─ mounted(repo_neighbor)
       -> [repo] Handoff selection flow
          path: .codex/context/playbooks/handoff-selection-flow.md

[thread] Simplify /context selection flow
  └─ covered_by
       -> [repo] Handoff selection flow

[thread] tui/src/chatwidget.rs
  └─ related(same_file)
       -> [thread] Simplify /context selection flow
```

The key UI takeaway is that the current thread can visibly contain:

- nodes created here
- nodes imported from the previous thread
- repo notes that already cover or neighbor the current work

without inventing a second graph system.

## Fork and Handoff Semantics

### Fork

If thread `B` is forked from thread `A`:

- the `anchor` for `B` sets:
  - `currentThreadId = B`
  - `precursorThreadId = A`
  - `precursorKind = fork`
- selected inherited nodes from `A` appear as `thread` nodes with:
  - `originThreadId = A`
- the `anchor -> node` edge uses:
  - `mountReason = fork_seed`

### Handoff

If thread `B` is created by handing off from thread `A`:

- the `anchor` for `B` sets:
  - `currentThreadId = B`
  - `precursorThreadId = A`
  - `precursorKind = handoff`
- selected handoff refs from `A` appear as `thread` nodes with:
  - `originThreadId = A`
- the `anchor -> node` edge uses:
  - `mountReason = handoff_seed`

This gives the UI enough information to highlight "from previous thread" without introducing separate memory classes.

## Accessing Other Threads

The graph should support discoverability without forcing every client to preload all thread artifacts.

Recommended behavior:

- `thread/read(threadId)`
  returns the full thread record and lineage metadata
- `context/query(currentThreadId = X, precursorThreadId = Y, ...)`
  returns the current rooted graph projection
- `context/query(currentThreadId = Y, ...)`
  returns a rooted projection for the previous thread directly
- `context/open(nodeId)`
  opens one specific node
- `context/hotspots(...)`
  returns notable nodes near the current working set

This keeps the graph queryable and discoverable while avoiding a prompt-sized dump of the whole repo graph.

## Promotion Model

Promotion should stay simple and safe:

- only `thread` nodes can be promoted
- promotion is additive by default
- repo notes are not silently overwritten

Suggested flow:

1. Agent selects one or more `thread` node IDs from its current rooted graph.
2. Agent calls `memory/promote`.
3. Server checks coverage against existing `repo` nodes.
4. Server either:
   - creates a new repo note
   - returns `alreadyCovered`
   - returns `proposalRequired`

Suggested request:

```json
{
  "currentThreadId": "thread-2",
  "selectedNodeIds": [
    "ctx:thread-insight:thread-2:plan-1"
  ]
}
```

Suggested response:

```json
{
  "created": [
    "ctx:file:.codex/context/hotspots/selection-footguns.md"
  ],
  "alreadyCovered": [
    "ctx:thread-insight:thread-1:plan-2"
  ],
  "proposalRequired": []
}
```

## Why This Model

This model is meant to keep the graph simple enough to reason about:

- only two real content node types
- provenance handled through metadata and edges
- no separate "self thread" and "lineage" storage systems
- one rooted graph view that works for humans and agents

It also fits the desired demo:

1. A dev works in a local thread.
2. They cherry-pick thread nodes.
3. They hand off to a connected agent.
4. The receiving agent gets a fresh anchor:
   - current thread = new thread
   - precursor thread = sender thread
5. Imported nodes are visibly marked as coming from the previous thread.
6. Repo notes remain mounted as shared durable knowledge.

## Current Mapping to Existing Code

This proposal is a simplification of the current implementation direction, not a claim that the code already matches it exactly.

Rough mapping:

- `thread` node payloads already exist as thread-derived `ContextDocument`s in [context-graph/src/lib.rs](../context-graph/src/lib.rs)
- `repo` node payloads already exist as repo context files in [context-graph/src/lib.rs](../context-graph/src/lib.rs)
- `source_thread_id` already exists on `ContextRef` in [together-protocol/src/lib.rs](../together-protocol/src/lib.rs)
- handoff already carries `source_thread_id` in [together-protocol/src/lib.rs](../together-protocol/src/lib.rs)

The main missing implementation pieces are:

- explicit anchor payloads in query responses
- explicit mounted/provenance edge kinds
- non-Codex agent ingestion APIs
- promotion as a first-class additive action over selected thread node IDs

## Implementation Phases

### Phase 1: Protocol Lock-In

- finalize the rooted graph protocol shape
- add external-agent thread-ingest protocol types
- add additive promotion protocol types
- keep current runtime behavior intact

Checkpoint:

- docs describe the rooted graph model
- protocol types exist and compile
- no server or TUI behavior has changed yet

### Phase 2: Shared Graph Engine

- update `codex-context-graph` to emit anchor/thread/repo projections
- add typed provenance edges
- keep current extraction logic for thread and repo artifacts

Checkpoint:

- the shared engine can build rooted graph projections from existing data

### Phase 3: Server Discovery and Promotion

- implement rooted graph queries in `codex-together-server`
- expose thread discovery and external-agent thread ingestion
- implement additive `memory/promote`

Checkpoint:

- server supports rooted graph queries and promotion over selected thread nodes

### Phase 4: TUI Demo Flow

- render the rooted graph in `/context`
- show provenance treatments like `here`, `from prev`, `handoff`, `repo`
- keep `/handoff` as addressed handoff to a connected agent

Checkpoint:

- the human demo flow works end-to-end in the TUI

### Phase 5: External Agent Integration

- add one thin non-Codex client
- ingest thread items directly into `codex-together-server`
- query rooted graph context and promote selected nodes

Checkpoint:

- at least one non-Codex agent runtime can participate in the same graph model

## Demo-Ready Implementation Checklist

This is the concrete work needed to get from the current branch state to a demo-ready version of this model.

### 1. `together-protocol`

Update the wire model so the rooted graph is explicit instead of implied.

Main changes:

- replace or extend `ContextGraphResponse`
  - add `anchor`
  - add typed graph nodes instead of only `ContextSearchResult`
  - add typed graph edges instead of string labels only
- add `ContextQueryParams` or evolve `ContextGraphParams`
  - `currentThreadId`
  - `precursorThreadId`
  - `precursorKind`
  - `seedRefIds`
  - `query`
  - `limit`
- add a promotion API that is additive and node-ID based
  - `memory/promote`
- add external-thread APIs for non-Codex agents
  - `thread/start`
  - `thread/appendItems`
  - `thread/read`
  - `thread/list`

Likely files:

- `together-protocol/src/lib.rs`

### 2. `context-graph`

Refactor the shared graph engine so it can emit the simplified rooted model.

Main changes:

- keep thread-derived and repo-derived document extraction
- add anchor construction helpers
- add typed graph node conversion:
  - `thread`
  - `repo`
- add provenance-aware edge generation:
  - `mounted`
  - `related`
  - `covered_by`
  - `promoted_to`
- add rooted traversal helpers from the current anchor
- keep hotspot scoring, but compute over the new typed graph

Likely files:

- `context-graph/src/lib.rs`

### 3. `together-server`

Make the server the canonical collaboration and discovery surface for the rooted graph.

Main changes:

- evolve `context/graph` into a rooted `context/query` style handler, or keep the method name and upgrade the response shape
- construct an explicit anchor from:
  - current thread
  - precursor thread
  - handoff/fork goal
- return mounted/provenance edges for the current working set
- expose thread discovery APIs:
  - list threads in repo
  - read one thread
  - inspect lineage/precursor relationships
- add ingestion for non-Codex agents so they can append normalized thread items
- keep handoff addressed to a connected agent for the demo
- change promotion from "write review" semantics to additive `memory/promote` semantics over selected thread nodes
- keep coverage detection and no-op/proposal behavior on overlap

Likely files:

- `together-server/src/lib.rs`
- possibly `state/*` if thread lineage and non-Codex thread metadata need persistence beyond the current app-server view

### 4. `core`

Keep `context_graph` native in Codex core, but make it emit the same logical model as the server.

Main changes:

- preserve local zero-config access for Codex agents
- synthesize an anchor from the current local thread
- optionally include precursor/fork metadata when available
- update tool output schema to align with the rooted graph model
- keep thread-derived artifacts and repo notes as the local data sources

Likely files:

- `core/src/tools/handlers/context_graph.rs`
- `core/src/tools/spec.rs`
- `codex-protocol` types if the native tool schema lives there

### 5. `app-server-protocol` and thread history

Thread provenance needs to remain reconstructible from history and rollout items.

Main changes:

- keep `ContextGraphQuery` reconstruction working with the new graph output model
- add precursor/fork/handoff lineage metadata to threads if not already present
- ensure thread items from non-Codex agents can be normalized into the same artifact model used by `context-graph`

Likely files:

- `app-server-protocol/src/protocol/thread_history.rs`
- any thread metadata types used by app-server v2

### 6. `tui`

Adapt `/context` and `/handoff` to the rooted graph and provenance labels.

Main changes:

- render the returned anchor as the current browsing root
- show per-node treatments derived from metadata:
  - `here`
  - `from prev`
  - `handoff`
  - `repo`
- keep the one-line selection UI
- keep `/handoff` as addressed handoff to a connected agent
- on the receiving thread, render imported nodes as mounted seeds from the precursor thread
- remove remaining user-facing concepts that assume the old write-review flow

Likely files:

- `tui/src/chatwidget.rs`
- `tui/src/app.rs`
- `tui/src/app_event.rs`
- `tui/src/chatwidget/tests.rs`
- TUI snapshots under `tui/src/chatwidget/snapshots/`

### 7. External Agent Clients

Non-Codex agents need a native integration path that does not depend on the Codex CLI UI.

Main changes:

- add a small Rust client if needed for first-party agent runtimes
- add at least one thin external client:
  - TypeScript or Python
- client responsibilities:
  - connect to `codex-together-server`
  - initialize/authenticate
  - start or resume a thread
  - append normalized thread items
  - query rooted graph context
  - promote selected nodes

This can start as a small library or examples folder if we want to keep the demo scope tight.

### 8. Docs and Demo Materials

Keep the docs aligned with the simplified graph model.

Main changes:

- keep this file as the canonical graph-shape reference
- update `docs/second-brain-demo-handoff.md` as the product-level source of truth
- once the RPC shape changes, document the concrete payloads and demo flow

## Suggested Implementation Order

If the goal is "demo-ready" rather than "fully generalized platform", the least risky order is:

1. finalize the rooted graph protocol shape
2. update the shared `context-graph` engine
3. update `together-server` query and promotion handlers
4. update the TUI `/context` and `/handoff` views
5. add non-Codex thread ingestion
6. add one thin external client

That order preserves the current Codex-first demo while opening the path for other agent runtimes immediately afterward.
