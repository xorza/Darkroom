# Scenarium

Scenarium is the node-graph framework: a serializable authoring model, a
compile → plan → execute pipeline, and an asynchronous worker. It depends only
on `common` in-tree. `lib.rs` is the public façade; implementation modules are
crate-private, so downstream crates import public concepts directly from
`scenarium`.

## Models and identities

The authoring `Graph` owns `Node`s keyed by `NodeId` plus side tables for input
bindings, event subscriptions, pinned outputs, and local graph definitions.
A `Graph` is an *entry* graph: no interface, not instantiable. A reusable
local/shared definition is a `GraphDef` — a `GraphInterface` (name, category,
ports, library lineage) plus the `Graph` `body` implementing it, so "a
definition has an interface" is a type fact rather than a validated invariant.
`GraphInterface` lives beside the identity and link types it composes, in
`graph/interface/`. `GraphDef` is deliberately not `Deref<Target = Graph>`: reach the
body through `body`, since a whole-value operation (`validate`,
serialization) must target the `GraphDef` — the body's own would silently skip
the interface.
Neither type is `Clone`: node and graph ids are document-unique, so every copy
declares its intent — `clone_mapped` remaps them (import, localize, detach,
publish), `clone_verbatim` preserves them (undo/redo replay, library
composition). Both names are the same on `Graph` and `GraphDef`.
Identity exists only in the map key; `Node` is authored data and does not store
its id. Its cache mode is storage policy, not cache validity. `Graph` is the
persisted model. `Graph::validate` enforces node-id *and* graph-id
uniqueness across the whole reachable authoring tree. Node removal and restoration
use `DetachedNode`, which keeps the id, node, all touching wiring, subscriptions,
and pins together.

Compilation produces a private, immutable `Program`. Composite nodes
are dissolved into flat function nodes stored in a dense, id-sorted vector
(`NodeIdx` positions) plus packed input, output-metadata, and event pools.
Each node stores typed ranges into those shared vectors, avoiding per-node
port allocations, and every compiled `Bind` edge is interned to a dense
`OutputAddr` — per-run scheduling, resolution, and execution state are
`NodeIdx`-aligned columns and bitsets — as is the cross-run `RuntimeCache`,
whose slots are re-paired with the new index order by stable id at each
install — so runs walk arrays without hashing ids. Top-level nodes retain
the UUID value of their authoring `NodeId` behind the distinct
`ExecutionNodeId` type; nested execution ids are derived with domain-separated
BLAKE3 from the enclosing instance ids and interior node id. Flatten records
each node's authored origin beside the node itself as it emits, so the one sort
that gives the program its dense order settles the attribution column with it:
one leaf per node, dense in the program's index space, over the compact scope
ancestry naming its enclosing instances. Nothing is keyed by execution id in
between, and the program's `e_node_index` is the artifact's only stable-id
index. Targeted runs and runtime reports
use exact `ExecutionNodeId`s at the host boundary (`NodeIdx` is install-local
and never enters a digest, persisted byte, or report); the host projects them
through the installed `CompiledGraph` when it needs authoring identities.

## Source layout

| Path | Responsibility |
| --- | --- |
| `data/type_system.rs` | `TypeId`, `DataType`, enum metadata, filesystem path configuration |
| `data/static_value.rs` | Serializable authored constants |
| `data/dynamic_value.rs` | Runtime values, custom values, and RAM accounting |

| `graph/error.rs` | What an authoring graph rejects: `GraphValidationError`, `GraphDeserializeError` |
| `graph/node/mod.rs` | The authored `Node` and its vocabulary: `NodeKind`, `CacheMode`, `NodeRef`, `NodeSearch` |
| `graph/node/error.rs` | `FuncValidationError` (registration) and `InvokeError`/`InvokeResult` (run time) |
| `graph/node/definition.rs` | Function declarations and port metadata |
| `graph/node/output_type.rs` | Shared wildcard-output type resolution |
| `graph/node/lambda.rs` | Function invocation ABI and output demand |
| `graph/node/event.rs` | Event-lambda ABI |
| `graph/mod.rs` | The `Graph` type, `Binding`/`BindingEntry`, the cycle check, and every `Graph` method in one impl |
| `graph/definition.rs` | The `GraphDef` type: its builders and every question asked of one |
| `graph/serde.rs` | Custom graph wire formats |
| `graph/validate.rs` | `Validator`: standalone and execution-entry graph validation |
| `graph/detached.rs` | The reversible-removal records (`DetachedNode`, `DetachedGraphInput`/`Output`) and their preflight asserts |
| `graph/clone.rs` | `MappedClone`, the result of an identity-remapping deep clone |
| `graph/boundary/` | `Shift`: how an interface-port edit renumbers the ports around it |
| `graph/query.rs` | The `NodePorts`/`NodeEvents` views a node's declaration resolves to |
| `graph/interface/` | Graph identity, instance links, exposed events, and the `GraphInterface` they compose |
| `execution/compile/` | Host-side compiler, linking (flat graph → program + indices), and the compiled artifact |
| `execution/compile/error.rs` | `CompileError` plus the artifact's self-consistency verdicts |
| `execution/error.rs` | Run-phase failures: whole-operation `Error`, per-node `RunError`, `ExecutionIdentityError` |
| `execution/flatten/` | Composite lowering into a stable-id `FlatGraph` |
| `execution/identity.rs` | Execution identities and compact authoring attribution |
| `execution/program/` | Private flat runtime program (construct-once) and typed packed pools |
| `execution/schedule/` | (with `error.rs` for its validation verdicts) The per-run `RunSchedule`, the `Scheduled`/`Resolved` phase handles, and every pass over it: the structural plan, the cache-aware sweep (liveness, reuse, demand, reader counts), and validation |
| `execution/executor/` | Invocation, delivery, reclamation, and outcomes |
| `execution/cache/` | The whole caching subsystem: cross-run values and output coverage, the content digests keying them, the filesystem identities those fold, and the on-disk blob store |
| `execution/codec/` | Streaming downstream custom-value codec API, and what it rejects |
| `execution/report.rs` | Internal live progress and pinned-output transport |
| `execution/outcome.rs` | Completed-run outcome and the public per-node status row it carries |
| `worker/protocol.rs` | Host/worker messages and reports |
| `worker/error.rs` | `WorkerError` and `WorkerExited` |
| `worker/status.rs` | Shared worker activity and node-status snapshots |
| `worker/batch.rs` | Ordered batch reduction |
| `worker/event_loop.rs` | Active event-task lifecycle |
| `worker/pause_gate/` | Counted RAII pause gate for worker execution |
| `worker/mod.rs` | Worker handle |
| `worker/task.rs` | Linear worker-task orchestration |

## Compile, plan, execute

`Compiler::compile` runs synchronously on the host and returns a
`CompiledGraph`; compilation is independent of run seeds. It is two stages, each
producing one value from the last: **flatten** lowers the authoring graph into a
`FlatGraph` — func-only, in the stable-id space, carrying everything it copied
out of the `Library` — and **link** places those nodes in the dense index space,
resolving every id-named reference (bindings, subscriptions, wildcard output
types) against that placement and building the host-facing indices over the
result. Flatten never names a dense index and link never sees the library, so
each of flatten's port types is the stage-local half of a program one
(`FlatInput`/`ExecutionInput`, and so on) rather than a program type with fields
left blank. Disabled leaves stay
in the program with an effective disabled bit inherited from composite
ancestors. Compile errors never enter the worker. Planning is structural: it
selects exact execution-node roots, treats those seeds as one-run disable
overrides, orders dependencies before consumers, and detects missing inputs.
Resolution refines that same
`RunSchedule` in place: it stamps content digests, then derives cache-aware
liveness, exact `OutputDemand`, and binding-reader counts together. The phases
hand each other typed handles rather than the buffer — `plan` returns a
`Scheduled`, `resolve` consumes it and returns a `Resolved`, and the executor
accepts only the latter — so phase order and program/schedule alignment are
compile-time facts rather than sequencing the engine has to get right. Execution invokes the surviving nodes in plan
order. Event-loop bootstrap marks subscribed event owners as event sources,
forces their initialization lambdas to run instead of reusing output caches,
and prepares triggers only for sources that complete successfully. The worker
takes those exact runtime triggers from `ExecutionOutcome` and moves them into
event tasks; fired-event runs do not rebuild unrelated triggers.

Before resolution, the `RuntimeCache` collects filesystem identities on Tokio's
blocking pool, through the `StampJob` it owns — a queue-then-walk pass that moves to
the pool and back, so nothing of the cache is borrowed across the boundary. The cache
memoizes each path for one run and reuses it for late bound-path restamps after
producers settle, keeping `node_digest` itself synchronous and I/O-free. The memo lives
on the cache because the two are always read together: a digest needs a path's
identity, and a path is identified off a producer's slot.

A cache slot is valid only when its digest matches and its
`OutputSnapshot` coverage contains every currently demanded output. Invocation
clears the output buffer first, so an output the lambda skips cannot retain a
stale value. Disk frames persist the same coverage; a same-digest write replaces
an older frame when the new result covers more outputs.

Only `FuncBehavior::Pure` cones receive reusable content digests. Filesystem-path
inputs fold the current referent's metadata identity. Explicit cache eviction is
a worker operation: authored ids resolve through `CompiledGraph`, expand through
transitive data consumers, release resident outputs, and delete their node-keyed
disk blobs. Custom runtime values receive disk-cache support only when their type
attaches a `CustomValueCodec`.

## Worker

Each worker wake drains the currently ready `WorkerMessage`s into a reusable
vector and reduces them as one commit unit. `BatchIntent` preserves first-seen
order while deduplicating node seeds and events; conflicting state slots are
last-write-wins and `Exit` dominates its batch. Compiled programs are shared as
`Arc<CompiledGraph>` values. After applying a graph-state change, the worker
emits `Installed` or `Cleared` before any report belonging to the resulting
state; its single execution loop and callback preserve that FIFO stream.
Successful cache eviction is fire-and-forget. Operation-level execution and
cache-eviction failures both arrive as `WorkerReport::Error`.
`WorkerReport::Status` carries an `Arc<WorkerStatus>` with absolute activity,
batched live node patches, or an authoritative completed-run snapshot. The
`WorkerStatusPublisher` retains one status allocation and updates it through `Arc::make_mut`;
the GUI consumes and drops published snapshots, allowing subsequent reports to
reuse their vectors when no older snapshot is still queued.
A completed run reaches the GUI as the rows the executor produced, unchanged:
`collect_outcome` reduces the per-node verdict column to exactly one `NodeStatus`
per node — its status, the ports it went unfed on, the error and time a failure
cost, and the RAM it kept — so publishing is an `append` and no consumer's fold
order can decide what a node's result was.
`WorkerTask` likewise retains one `ExecutionOutcome`; the engine clears and
repopulates its buffers for each run, then completion drains them into the
status publisher without discarding their capacities.
`ActiveEventLoop` owns both its tasks and event receiver, so the activity
invariant is represented by one type. Event tasks rendezvous through Tokio's
`Barrier`; the worker's counted pause gate uses Tokio `watch` so overlapping
close guards reopen it only after the last guard drops. Worker reports stream
node-status patches and exact scoped pinned outputs before the matching
completion snapshot.

## Tests

Test fixtures and private-state builders are available only under tests or the
`internals` feature; downstream crates enable `internals` only as a dev
dependency. Test helpers stay in gated `internals` modules beside the
private state they access.
