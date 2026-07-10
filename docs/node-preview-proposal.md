# Proposal: execute a specific node for preview

Goal: execute a chosen node (and its upstream cone) on demand, cache its outputs
in RAM, and preview them in darkroom — via an explicit GUI action and/or
automatically when a preview is requested.

## TL;DR

Almost everything needed already exists. Scenarium already has digest-keyed RAM
caching (`ValueState::Resident` in `scenarium/src/execution/cache/mod.rs`), a
planner that only schedules the sub-DAG upstream of its roots, and a pull API
darkroom already uses to fetch cached values (`WorkerMessage::RequestArgumentValues`).
Darkroom already has the whole watch→request→ingest→display loop (`RunState`,
image-viewer tabs, inspector thumbnails). **The single missing primitive is a
run seed that targets an arbitrary node** — today `RunSeeds`
(`scenarium/src/execution/mod.rs`) can only say "all terminals" or "event
subscribers". Add that seed plus a small retention rule, wire one new command
through darkroom, and both an explicit "run to this node" action and
auto-run-on-preview fall out with essentially no new value plumbing.

## Part 1 — scenarium: a node-targeted run seed

**1. `RunSeeds.nodes: Vec<NodeId>`.** The three existing seed kinds combine
freely; a fourth composes the same way. In `collect_roots`
(`scenarium/src/execution/plan/mod.rs`), resolve each authoring id to a flat
`NodeIdx` and push it into `plan.roots`. The resolution precedent already
exists: `resolve_query_idx` (`scenarium/src/execution/query.rs`) is exactly
what `RequestArgumentValues` uses to map an authoring id to a flat node
(top-level ids map to themselves; subgraph-interior ids resolve to the first
instance). The planner's backward DFS, the resolver's cache-aware cone cut, the
executor loop, and the digest cache all work unchanged on any root set — so a
preview run automatically becomes *incremental*: unchanged upstream nodes with
resident/disk values are served as cache hits and only the dirty cone
re-executes.

**2. `WorkerMessage::ExecuteNodes { nodes: Vec<NodeId> }`** alongside
`ExecuteTerminals` (`scenarium/src/worker/mod.rs`). Darkroom sends it batched
with `Update` exactly like `run_once` does today, so the run always executes
against the current graph. The worker's `BatchIntent` reduction table needs a
rule: union node sets; `ExecuteTerminals` composes (seeds combine).

**3. The retention rule** — the subtle part. A seeded root has zero consumers,
which today means two things kill the preview value:

- Its outputs get `OutputUsage::Skip` (`scenarium/src/execution/executor/mod.rs`,
  usage-column init), so a well-behaved lambda may not compute them at all.
- Even if computed, a non-RAM node with all-Skip outputs is reclaimed **the
  instant it's stored** (same file, post-invoke reclaim), and `evict_unused`
  (`scenarium/src/execution/cache/mod.rs`) sweeps any leftovers after the run.

So the seed must carry a per-run pin for its target nodes, folded into one
predicate: the executor's `retain` column (`caches_in_ram() || pinned`), which
every free/keep decision consults — the move-on-last-use take, the
spent-output release, the post-invoke drain reclaim, and `evict_unused`. The
usage counts stay honest bookkeeping of in-run reads; the only pin-specific
usage effect is a `Needed(1)` floor on outputs with zero in-run consumers,
because the preview fetch reads them after the run. Retention is all it takes
for a repeated preview to be free: RAM reuse trusts residency alone — a
content digest attests the value produced under it, however it came to be
resident (mode retention, pin, or inspection hydration) — and cache modes act
only on what *stays* resident (drain, eviction). (The one node type that broke
this uniformity, the path-keyed `CachePassthrough` file cache, has since been
removed.) No document mutation, no persistent `CacheMode` change — the value
just survives the run. It only needs to survive long enough
for the GUI's value fetch anyway: `PortValueView::full`
(`darkroom/src/gui/node_values.rs`) retains its own `Arc` clone of the image,
so later eviction by a subsequent run is harmless.

**Status: phase 1 implemented** — `RunSeeds.nodes`, `ExecutionPlan.pinned`,
`WorkerMessage::ExecuteNodes`, residency-as-single-source-of-truth reuse, and
tests at every layer (planner, executor, engine, composite interior, worker
end-to-end).

## Part 2 — darkroom: plumbing plus two affordances

**Plumbing (mechanical):** add `RunCommand::Node(NodeId)`
(`darkroom/src/gui/app/commands/run.rs`) → `App::handle_run` calls
`run_state.begin_run()` + a new `Engine::run_node`
(`darkroom/src/core/engine.rs`) → `WorkerBridge::run_node` sending
`[Update, ExecuteNodes]`. That's it. Everything downstream — progress spinners,
epoch bump, the per-frame watch registry re-requesting values, inspector
thumbnails, image-viewer refresh via `sync_image_viewers`
(`darkroom/src/gui/app/editor/mod.rs`) — works **unchanged**, because a preview
run is just a run.

**Affordance A — explicit "Run to this node" (recommended first step).** Add it
to the node right-click menu: `NodeMenuAction`
(`darkroom/src/gui/canvas/node_menu.rs`) currently has
Duplicate/DuplicateWithIncoming/Remove; the action drains through
`Editor::apply_node_menu_action` which can surface the `AppCommand`. This path
already targets the clicked node and has the document in hand. As a phase-2
nicety, a small play chip in the node header's `status_row`
(`darkroom/src/gui/node/header.rs`, next to the existing `D`/`R`/`↓` chips)
gives one-click access; the chip infrastructure with deterministic `WidgetId`s
is already there.

**Affordance B — auto-run when a preview is requested.** The image-viewer tab
is the right trigger: opening or activating one is an unambiguous "I want to
see this output" signal (inspectors are too casual — you'd fire computation on
every `i`-chip click). `Editor::sync_image_viewers` already walks live viewer
tabs each frame and knows when a viewer has no value for the current epoch;
extend it to emit `RunCommand::Node(port.node_id)` when: the viewer's port has
no value for the current `run_id`, no run is in flight, and we haven't already
attempted for this cause. The loop guard matters — if the run fails upstream,
the value stays missing and a naive "missing → run" rule retriggers forever.
Guard it as *one attempt per tab activation* (record the attempted `RunId` per
viewer, reset on tab switch or document edit). Thanks to digest caching, a
redundant auto-run of an unchanged graph costs nearly nothing, so the guard is
about predictability, not performance.

## Edge cases

- **Subgraph-interior nodes**: resolve to the first flat instance, same as
  `RequestArgumentValues` does today. Fine for v1.
- **Disabled nodes**: flattened out of the program — the menu item must be
  disabled for them; a seed that doesn't resolve fails the run with
  `Error::NodeSeedNotFound` (seeds are batched with the graph they target, so a
  miss is inconsistent caller state).
- **Missing upstream inputs**: the planner already verdicts these
  `MissingInputs`; existing status reporting covers it.
- **Value blanking**: `begin_run` nulls all displayed values for the new epoch;
  watched nodes re-fetch automatically, but un-cached nodes outside the preview
  cone will show empty — same behavior as any run today.

## Phasing

1. Scenarium: `RunSeeds.nodes` + `ExecuteNodes` + the retention pin, with tests
   (seed resolution, output-usage pinning, retention through `evict_unused`,
   cache-hit cone cut). **Done.**
2. Darkroom: `RunCommand::Node` plumbing + context-menu item — this alone
   delivers the feature. **Done** — `WorkerBridge::run_node` →
   `Engine::run_node` → `RunCommand::Node`; the menu item resolves in
   `NodeMenuUi` itself (it only needs the clicked node's id, routed through
   the canvas's `AppCommand` channel like the subgraph menu's Publish) and is
   omitted for disabled nodes and subgraph instances, since neither resolves
   as a seed.
3. Auto-run in `sync_image_viewers`. **Implemented, then removed by
   decision** — the explicit menu action is the only trigger. Lessons from
   the implementation, should it come back: the sketched per-activation
   `RunId` guard can't work as written (every auto-run bumps the epoch, so a
   failed run reads as a fresh cause and retriggers forever) — mark the
   attempt with the *epoch the kicked run creates* instead, reset on document
   edits; also gate on the node's last-run status being `ExecStatus::None`,
   since `Executed`/`Cached` means the watched value fetch is already en
   route and firing would race it; and only ever fire for the *active*
   viewer tab — background tabs endlessly re-run each other's cones, since
   each preview run evicts the previous run's unpinned values.
4. Optional polish (not done): header play chip, keyboard shortcut on
   selection.
