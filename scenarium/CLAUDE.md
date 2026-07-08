# scenarium

The node-graph framework: an authoring graph model, a compile→plan→execute
pipeline that flattens composites and schedules async node functions, and a
tokio worker that drives runs and an event loop. Depends on `common`; no other
in-tree deps. Public surface is re-exported from `prelude` (`lib.rs:16`).

## Two graph models

The crate has **two** distinct graph representations and the pipeline lowers one
to the other:

- **Authoring graph** (`graph.rs`, `subgraph.rs`) — what the editor edits.
  Nodes, bindings, subscriptions, nested composites. Serializable, identity-only
  nodes.
- **Execution program** (`execution/program.rs`) — the compiled, flattened,
  immutable form. Composites are dissolved; everything is flat func nodes with
  SoA pools. The only serializable runtime artifact.

## Module layout

| Path | Role |
|------|------|
| `graph.rs` | `Graph`, `Node`, `Binding`, `Subscription`, ports, `NodeKind`, `CacheMode`. Authoring model + validation. |
| `subgraph.rs` | `SubgraphDef`, `SubgraphRef` (Linked/Local), `SubgraphEvent`. Composite definitions and their exposed interface. |
| `function.rs` | `Func` (definition), `FuncInput`/`FuncOutput`, `FuncBehavior`. |
| `library.rs` | `Library` (the registry: funcs + shared subgraphs + nominal types), `TypeDecl`/`TypeEntry` (type metadata + optional disk codec). |
| `data.rs` | Value model: `StaticValue` (editor consts), `DynamicValue` (runtime), `DataType` (`Custom`/`Enum` carry only a `TypeId`; metadata lives on `Library`), `CustomValue` trait. |
| `context.rs` | `ContextManager` (per-run resource store + log sink + the run's cancel flag via `cancel_flag()`, which a lambda clones into off-thread work to bail early), `ContextType` (lazy-init type token). |
| `func_lambda.rs` | `FuncLambda`: the async node-function signature + `InvokeInput`/`InvokeResult`/`InvokeError`. |
| `event_lambda.rs` | `EventLambda`: async event-handler signature. |
| `macros.rs` | `async_lambda!` — ergonomic `FuncLambda` construction. |
| `elements/` | Built-in node libraries: `math_library.rs` (Math), `system_library.rs` (System: print / to-string / concat), `worker_events_library.rs` (System: frame/fps events), `fs_watch_library.rs` (System: dir watch), `cache_passthrough.rs` (System: file cache), `run_terminals.rs` (System: "Run on Event" — a portless terminal sink that re-runs all terminals when a subscribed event fires). |
| `execution/` | The compile→plan→execute pipeline (see below). |
| `execution_stats.rs` | `ExecutionStats` per-run summary + `FlattenMap` (flat id → authoring attribution), `LogEntry`. |
| `common/` | `AnyState` (per-node mutable state), `SharedAnyState` (concurrent event state). |
| `worker/` | `Worker`: tokio task driving updates, runs, and the event loop. |
| `testing.rs` | `test_func_lib` fixtures with pluggable hooks. |

## Authoring graph (`graph.rs`)

`Graph` (`graph.rs:173`) uses **side-table storage** — nodes carry no per-node
port Vecs; wiring lives flat at graph level:
- `nodes: KeyIndexVec<NodeId, Node>` — insertion order preserved.
- `bindings: BTreeMap<InputPort, Binding>` — sparse, deterministic data wiring.
- `subscriptions: BTreeSet<Subscription>` — deduped event edges `(emitter, event_idx, subscriber)`.
- `subgraphs: KeyIndexVec<SubgraphId, SubgraphDef>` — *local* composite defs.

`Node` (`graph.rs:126`) is pure identity: `id`, `kind: NodeKind`
(Func / Subgraph / SubgraphInput / SubgraphOutput), `name`,
`cache: CacheMode` (`None | Ram | Disk | Both`), `disabled: bool`. Port arity
derives from `kind`. `Binding` (`graph.rs:48`) is `None` (unbound) /
`Const(StaticValue)` / `Bind(OutputPort)`. `validate_with(library)` recurses per
composite level with recursion guards.

A **subgraph** (`subgraph.rs`) has a `SubgraphDef` whose interior `graph` may hold
one `SubgraphInput` and one `SubgraphOutput` boundary node (routing only, not
executable). `SubgraphRef` resolves either `Linked` (shared, in `Library.subgraphs`)
or `Local` (private, in `Graph.subgraphs`). `SubgraphEvent` re-exposes an interior
emitter's event outward so a parent can subscribe.

## Value model (`data.rs`)

- `StaticValue` (`data.rs:118`) — serializable editor constants (Null/Float/Int/Bool/String/FsPath/Enum), NaN-aware equality.
- `DynamicValue` (`data.rs:173`) — runtime values, adds `None` (unbound) and `Custom(Arc<dyn CustomValue>)`; coercions `as_f64`/`as_i64`/`as_bool`.
- `DataType` (`data.rs`) — port type spec; `Custom`/`Enum` reference a registered nominal type by `TypeId` (name/variants/codec live on `Library`), `FsPath` stays inline. `CustomValue` (`data.rs`) is the app-extension trait (`type_id`, async `gen_preview`, `as_any`).

## Execution pipeline (`execution/`)

Three phases, driven by `ExecutionEngine` (`execution/mod.rs`):

**1. Compile — flatten authoring `Graph` → `ExecutionProgram`** (`flatten.rs`).
Composites dissolve; only interior func nodes survive, with deterministically
remapped ids. `flatten_id` (`flatten.rs:172`) keeps **top-level** node ids
unchanged (so caches survive edits) and hashes nested ids from the descent path +
interior id. `Flattener` (`flatten.rs:43`) walks levels, skipping `disabled`
nodes and boundary nodes, recursing into subgraph instances; bindings and event
edges are resolved across boundaries into flat producers/subscribers. Each flat
node copies its func's `FuncBehavior` (`Pure`/`Impure`) — only `Pure` is
content-cacheable.

`ExecutionProgram` (`program.rs:128`) is immutable SoA: `e_nodes`,
plus flat `inputs`/`events` pools indexed by `Span` slices per node; outputs are
span-only. `ExecutionBinding` (`program.rs:26`) is `None`/`Const`/`Bind(address)`.

**2. Plan — `Planner::plan()` → `ExecutionPlan`** (`plan.rs`).
Reusable `Planner` (`plan.rs`) is **purely structural** — no cache/digest state.
One backward DFS: collect terminals (terminal nodes, event subscribers, triggerable
events) → post-order `process_order` (deps first) — the single schedule, every reachable
node, producer-first. A fired event whose subscriber is a `RunTerminals` special node
(`SpecialNode::RunTerminals`) is promoted to a full terminal run — that node has no cone of
its own; its effect is "when this event fires, re-run every terminal" (`collect_roots`). Whether a node *reuses* a cache or recomputes, and whether it's
skipped (`MissingInputs`), is decided at execution, not here. `NodeVerdict` (`plan.rs`)
is just `Execute`/`MissingInputs`. `ExecutionPlan` holds `process_order` + `verdicts` +
`output_usage` (per-output consumer counts; 0 = skip) + `roots` (the walk roots, handed to
the executor's pre-run cut).

**3. Execute — `Executor::run(program, plan)`** (`executor.rs`).
`Executor` owns the `ctx_manager` + per-run scratch (invoke buffers + one `outcomes`
column of [`NodeOutcome`], each `Pending`/`Reused`/`Cut`/`Ran`/`Failed`/`Skipped` carrying
its own elapsed/error); the cross-run `slots` live on the `RuntimeCache`. `RuntimeSlot` (`cache.rs`)
caches `output_values` + the `produced_under` digest, per-node `AnyState`/`SharedAnyState`.

**One output digest.** The whole cache keys off a single `RuntimeSlot::current_digest`
(`digest.rs`). `node_digest` folds func id/version + output types + each input (a `Const`'s
value + `FsPath` directory content, or a `Bind` producer's already-stamped `current_digest`).
`None` for an `Impure` node (never cached; a `None` producer taints its consumer to `None`).
The one special case: a `CachePassthrough` (file-cache) node is keyed on its `Const` path
*alone* (`file_cache_digest`), excluding its `input[0]` cone. Reuse is uniform: a resident
value whose `produced_under == current_digest` (`is_resident_hit`) is served from RAM; else
`RuntimeCache::mark_on_disk_if_present` stats a blob for that digest and reuses it (loaded
lazily by `hydrate_slot` only when a running consumer reads it, so a disk-cached value behind
another never enters RAM — this is what survives a reopen); else the node runs and
`store_node` writes the blob. Downstream skip is just digest folding: a producer whose
`current_digest` is unchanged leaves its consumers' digests unchanged. **Pre-run cut**
(`resolve.rs`): before the run loop the executor resolves *every* node's digest + reuse
(`resolve_structural` — folding each digest producer-first from upstream digests, no lambda,
no value load — every digest being structural or the file-cache path key) and prunes every
cone that feeds *only* reuse hits (`compute_needed`, a backward walk seeded from the plan's
`roots`), so a `Memory` (non-persist) node feeding a disk-cached *hit* is **not** recomputed
on reopen. A cut node is reported `cached` iff it still holds a value (a deeper disk cache),
else it's not computed this run. The run loop then re-derives each surviving node's digest
idempotently (`prepare_node`). Output buffers aren't wiped; `evict_unused` demotes to disk
only values the run's *executed* nodes didn't produce/read. A reused node counts as `cached`
in stats. When `execute` is given
a progress `UnboundedSender<RunProgress>`, the loop sends `RunPhase::Started`
before each lambda and `Finished{elapsed}` after — node ids resolved to authoring
attribution via the `FlattenMap` so the consumer needn't be. Stats (executed,
cached, missing inputs, errors, drained logs, `FlattenMap`) are collected into
`ExecutionStats`.

## Functions and lambdas

A `Func` (`function.rs:56`) is a definition (id, name, behavior, inputs/outputs/
events, `required_contexts`, and a runtime-attached `lambda` skipped on serialize).
Build one with the fluent builder — `Func::new(id, name).category(..).pure()
.input(FuncInput::required(name, ty)).output(name, ty).lambda(..)` — rather than a
struct literal (`FuncInput::required`/`optional(..).default(v)`, `FuncOutput::new`;
fields stay `pub` for serde + the editor). `Library` (`library.rs`) registers
funcs + shared subgraph defs + nominal types (`Custom`/`Enum`, each with optional
disk codec — `register_type`/`type_decl`/`codec`); the output cache's custom-value
codecs live on its `types` table. A node
function is a `FuncLambda` (`func_lambda.rs:62`): async
`fn(&mut ContextManager, &mut AnyState, &SharedAnyState, &[InvokeInput], &[OutputUsage], &mut [DynamicValue]) -> InvokeResult<()>`.
Build them with `async_lambda!` (`macros.rs`). `EventLambda` (`event_lambda.rs`)
is the async `fn(SharedAnyState)` event handler. `elements/` ships the built-in
node libraries.

## Worker (`worker/mod.rs`)

`Worker` (`worker/mod.rs:79`) wraps a tokio task fed by `UnboundedSender<Vec<WorkerMessage>>`.
A `Vec<WorkerMessage>` is **one atomic commit unit** — no partial batches.
`WorkerMessage` covers `Update{graph, library}`, `Clear`, `ExecuteTerminals`,
`InjectEvents`, `Start/StopEventLoop`, `Sync`, `RequestArgumentValues`,
`SetDiskCache(Option<DiskCache>)` (swap the engine's disk cache — applied before
any same-batch graph op so the next `Update` hydrates from the new store),
`Exit`. Disk caching is set initially via `Worker::new(disk_cache, callback)` and
repointed at runtime via `SetDiskCache` (e.g. a per-document cache dir).
The host callback receives a `WorkerReport`: a live `Progress(RunProgress)` per
node *during* a run (forwarded from a `mpsc` the executor sends on, drained
concurrently with the run in the worker `select!`), then a single
`Finished(Result<ExecutionStats>)`. **Cancel** is a shared `common::CancelToken`
on the `Worker` (`request_cancel()` sets it, the worker `reset()`s it at each
run's start) that the executor polls between nodes — set directly across threads, so
no command-channel round-trip; a cancelled run stops scheduling and reports
`ExecutionStats { cancelled: true }` with only the nodes that ran. The node that
was *mid-invoke* when the cancel landed is reported truthfully as
`Error::Cancelled`, not a fake success: its output is dropped so it isn't cached
(re-runs next time) and it's omitted from `executed_nodes`. Two routes produce it:
a cancellable lambda returns `InvokeError::Cancelled` (mapped to `Error::Cancelled`
rather than `Error::Invoke`); and as a fallback, a lambda that *doesn't* poll the
token but returned `Ok` while the run was cancelled is mapped to `Cancelled` too
(its result is from an aborted run). A genuine `Err` stands on its own, even
mid-cancel. darkroom paints a cancelled node neutrally (it was interrupted, not a
failure), unlike a real `Error`.
The loop uses `tokio::select! { biased }`: command batches take priority and are
collapsed via a `BatchIntent` reduction table (last-write-wins for graph/loop,
union for events, `Exit` dominates); the event loop emits frame events through a
bounded (backpressured) channel. Each active event runs as its own looping
`EventTrigger` task. **Cancel-safety**: `Update`/`Clear`/`StopEventLoop`/`Exit`
tear down the event loop and abort lambda tasks, so lambdas must hold no cleanup
state across `.await`.

## Tests

In-tree tests live in `execution/tests.rs` and `worker/tests.rs`; fixtures in
`testing.rs`. Inspection helpers are `#[cfg(test)]`-gated.
