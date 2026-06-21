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
| `graph.rs` | `Graph`, `Node`, `Binding`, `Subscription`, ports, `NodeKind`/`NodeBehavior`. Authoring model + validation. |
| `subgraph.rs` | `SubgraphDef`, `SubgraphRef` (Linked/Local), `SubgraphEvent`. Composite definitions and their exposed interface. |
| `function.rs` | `Func` (definition), `FuncLib` (registry of funcs + shared subgraphs), `FuncInput`/`FuncOutput`, `FuncBehavior`. |
| `data.rs` | Value model: `StaticValue` (editor consts), `DynamicValue` (runtime), `DataType`, `CustomValue` trait, `TypeDef`/`EnumDef`. |
| `context.rs` | `ContextManager` (per-run resource store + log sink), `ContextType` (lazy-init type token). |
| `func_lambda.rs` | `FuncLambda`: the async node-function signature + `InvokeInput`/`InvokeResult`/`InvokeError`. |
| `event_lambda.rs` | `EventLambda`: async event-handler signature. |
| `macros.rs` | `async_lambda!` — ergonomic `FuncLambda` construction. |
| `elements/` | Built-in funclibs: `basic_funclib.rs` (math/string/print), `worker_events_funclib.rs` (frame/fps events). |
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
`behavior: NodeBehavior` (`AsFunction | Once`), `disabled: bool`. Port arity
derives from `kind`. `Binding` (`graph.rs:48`) is `None` (unbound) /
`Const(StaticValue)` / `Bind(OutputPort)`. `validate_with(func_lib)` recurses per
composite level with recursion guards.

A **subgraph** (`subgraph.rs`) has a `SubgraphDef` whose interior `graph` may hold
one `SubgraphInput` and one `SubgraphOutput` boundary node (routing only, not
executable). `SubgraphRef` resolves either `Linked` (shared, in `FuncLib.subgraphs`)
or `Local` (private, in `Graph.subgraphs`). `SubgraphEvent` re-exposes an interior
emitter's event outward so a parent can subscribe.

## Value model (`data.rs`)

- `StaticValue` (`data.rs:118`) — serializable editor constants (Null/Float/Int/Bool/String/FsPath/Enum), NaN-aware equality.
- `DynamicValue` (`data.rs:173`) — runtime values, adds `None` (unbound) and `Custom(Arc<dyn CustomValue>)`; coercions `as_f64`/`as_i64`/`as_bool`.
- `DataType` (`data.rs:105`) — port type spec; `CustomValue` (`data.rs:36`) is the app-extension trait (`type_def`, async `gen_preview`, `as_any`).

## Execution pipeline (`execution/`)

Three phases, driven by `ExecutionEngine` (`execution/mod.rs`):

**1. Compile — flatten authoring `Graph` → `ExecutionProgram`** (`flatten.rs`).
Composites dissolve; only interior func nodes survive, with deterministically
remapped ids. `flatten_id` (`flatten.rs:172`) keeps **top-level** node ids
unchanged (so caches survive edits) and hashes nested ids from the descent path +
interior id. `Flattener` (`flatten.rs:43`) walks levels, skipping `disabled`
nodes and boundary nodes, recursing into subgraph instances; bindings and event
edges are resolved across boundaries into flat producers/subscribers. A composite
with `NodeBehavior::Once` bumps `once_depth` on entry (`flatten.rs:225`); while
`> 0`, every interior func is forced to `ExecutionBehavior::Once` — freezing the
whole composite after its first run.

`ExecutionProgram` (`program.rs:128`) is immutable SoA: `e_nodes`,
plus flat `inputs`/`events` pools indexed by `Span` slices per node; outputs are
span-only. `ExecutionBinding` (`program.rs:26`) is `None`/`Const`/`Bind(address)`.

**2. Plan — `Planner::plan()` → `ExecutionPlan`** (`planner.rs`, `plan.rs`).
Reusable `Planner` (`planner.rs:43`) runs backward DFS passes: collect terminals
(terminal nodes, event subscribers, triggerable events) → post-order
`process_order` (deps first) → forward pass resolving cached / wants-execute →
pruned `execute_order` (only nodes whose output a running consumer reads).
`ExecutionPlan` (`plan.rs:24`) holds the two orders plus SoA flag columns:
`node_flags`, `input_flags`, and `output_usage` (per-output consumer counts;
0 = skip).

**3. Execute — `Executor::run(program, plan)`** (`executor.rs`).
`Executor` (`executor.rs:53`) owns cross-run state: `slots: KeyIndexVec<NodeId, RuntimeSlot>`
(reconciled by id against `e_nodes` after each flatten), the `ContextManager`, and
the cross-run `input_dirty` column. `RuntimeSlot` (`executor.rs:29`) caches
`output_values`, per-node `AnyState`/`SharedAnyState`, and per-run `error`/`run_time`.
The run loop walks `execute_order`: skip if an upstream errored, resolve each input
(None/Const/Bind→upstream cached output, marking `changed` from dirty bit or
dependency wants-execute), set `ctx_manager.current_node` for log attribution,
await the lambda, store results, clear input dirty bits. When `execute` is given
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
fields stay `pub` for serde + the editor). `FuncLib` (`function.rs:87`) registers
funcs + shared subgraph defs. A node
function is a `FuncLambda` (`func_lambda.rs:62`): async
`fn(&mut ContextManager, &mut AnyState, &SharedAnyState, &[InvokeInput], &[OutputUsage], &mut [DynamicValue]) -> InvokeResult<()>`.
Build them with `async_lambda!` (`macros.rs`). `EventLambda` (`event_lambda.rs`)
is the async `fn(SharedAnyState)` event handler. `elements/` ships the built-in
funclibs.

## Worker (`worker/mod.rs`)

`Worker` (`worker/mod.rs:79`) wraps a tokio task fed by `UnboundedSender<Vec<WorkerMessage>>`.
A `Vec<WorkerMessage>` is **one atomic commit unit** — no partial batches.
`WorkerMessage` covers `Update{graph, func_lib}`, `Clear`, `ExecuteTerminals`,
`InjectEvents`, `Start/StopEventLoop`, `Sync`, `RequestArgumentValues`, `Exit`.
The host callback receives a `WorkerReport`: a live `Progress(RunProgress)` per
node *during* a run (forwarded from a `mpsc` the executor sends on, drained
concurrently with the run in the worker `select!`), then a single
`Finished(Result<ExecutionStats>)`.
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
