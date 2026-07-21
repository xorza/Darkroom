# Scenarium

Scenarium is the node-graph framework: a serializable authoring model, a
compile → plan → execute pipeline, and an asynchronous worker. It depends only
on `common` in-tree. `lib.rs` is the public façade; implementation modules are
crate-private, so downstream crates import public concepts directly from
`scenarium`.

## Models and identities

The authoring `Graph` owns `Node`s keyed by `NodeId` plus side tables for input
bindings, event subscriptions, pinned outputs, and local graphs.
Identity exists only in the map key; `Node` is authored data and does not store
its id. `Graph` is the persisted model. `Graph::validate` enforces node-id
uniqueness across the whole reachable authoring tree. Node removal and
restoration use `DetachedNode`, which keeps the id, node, all touching wiring,
subscriptions, and pins together.

Compilation produces a private, immutable `ExecutionProgram`. Composite nodes
are dissolved into flat function nodes and SoA pools. Top-level nodes retain
their `NodeId`; nested flat ids are derived with domain-separated BLAKE3.
`FlattenMap` maps both directions between flat ids and exact `NodeAddress`
values. A `NodeAddress` contains the graph-instance path plus the interior
node id; targeted runs and pinned-output delivery must use this scoped identity.
Choosing a representative instance is an explicit host-side policy.

## Source layout

| Path | Responsibility |
| --- | --- |
| `data/type_system.rs` | `TypeId`, `DataType`, enum metadata, filesystem path configuration |
| `data/static_value.rs` | Serializable authored constants |
| `data/dynamic_value.rs` | Runtime values, custom values, and RAM accounting |
| `data/resource.rs` | External-resource stamps and stampers |
| `node/definition.rs` | Function declarations and port metadata |
| `node/output_type.rs` | Shared wildcard-output type resolution |
| `node/lambda.rs` | Function invocation ABI and output demand |
| `node/event.rs` | Event-lambda ABI |
| `graph/mod.rs` | Core authoring structs, construction, and serialization entry points |
| `graph/serde.rs` | Custom graph wire formats |
| `graph/validate.rs` | Standalone and execution-entry graph validation |
| `graph/wiring.rs` | Wiring mutation, scoped node detach/attach, cycle checks |
| `graph/clone.rs` | Deep copies with fresh node ids |
| `graph/normalize/` | Recursive local-interface normalization and stale-library wiring cleanup |
| `graph/query.rs` | Type and reachability queries |
| `graph/interface/` | Graph identity, instance links, and exposed events |
| `execution/compile.rs` | Host-side compiler and compiled artifact |
| `execution/flatten/` | Composite lowering |
| `execution/identity.rs` | Scoped authoring addresses and flatten provenance |
| `execution/program.rs` | Private flat runtime program |
| `execution/plan/` | Structural scheduling and missing-input verdicts |
| `execution/resolve/` | Cache-aware liveness, reuse, output demand, and reader counts |
| `execution/executor/` | Invocation, delivery, reclamation, and stats |
| `execution/cache/` | Cross-run values and output coverage |
| `execution/codec.rs` | Cache framing plus downstream custom-value codec API |
| `execution/disk_store/` | On-disk cache persistence |
| `execution/report.rs` | Live progress and pinned-output transport |
| `execution/resource/` | Off-thread, memoized per-run external-resource stamps |
| `execution/stats.rs` | Completed-run results |
| `worker/protocol.rs` | Host/worker messages and reports |
| `worker/batch.rs` | Ordered batch reduction |
| `worker/event_loop.rs` | Active event-task lifecycle |
| `worker/pause_gate/` | Counted RAII pause gate for worker execution |
| `worker/mod.rs` | Worker handle and task orchestration |

## Compile, plan, execute

`Compiler::compile` runs synchronously on the host and returns a
`CompiledGraph`; compilation is independent of run seeds. Disabled leaves stay
in the program with an effective disabled bit inherited from composite
ancestors. Compile errors never enter the worker. Planning is structural: it
selects roots, treats explicit node seeds as one-run disable overrides, orders
dependencies before consumers, and detects missing inputs. Resolution stamps
content digests, then derives cache-aware liveness, exact `OutputDemand`, and
binding-reader counts together. Execution invokes the surviving nodes in plan
order.

Before resolution, `RunResourceStamps` collects filesystem identities and custom
resource stamps on Tokio's blocking pool. It memoizes each resource for one run
and is reused by late bound-resource restamps after producers settle, keeping
`node_digest` itself synchronous and I/O-free.

A cache slot is valid only when its digest matches and its
`OutputSnapshot` coverage contains every currently demanded output. Invocation
clears the output buffer first, so an output the lambda skips cannot retain a
stale value. Disk frames persist the same coverage; a same-digest write replaces
an older frame when the new result covers more outputs.

Only `FuncBehavior::Pure` cones receive reusable content digests. Resource
inputs fold the current referent identity through `ResourceStamper`; filesystem
paths have built-in stamping. Stampers receive the run's cooperative
`CancelToken`. Custom runtime values are not serializable.
Downstream types attach a `CustomValueCodec` explicitly when they want disk
cache support.

## Worker

One `Vec<WorkerMessage>` is one commit unit. `BatchIntent` preserves first-seen
order while deduplicating node seeds and events; conflicting state slots are
last-write-wins and `Exit` dominates its batch. `ActiveEventLoop` owns both its
tasks and event receiver, so the lifecycle invariant is represented by one
type. Event tasks rendezvous through Tokio's `Barrier`; the worker's counted
pause gate uses Tokio `watch` so overlapping close guards reopen it only after
the last guard drops. Worker reports stream progress and exact scoped pinned
outputs before the matching finished result.

## Tests

Test fixtures and private-state builders are available only under tests or the
`internals` feature; downstream crates enable `internals` only as a dev
dependency. Test helpers stay in gated `test_support` modules beside the
private state they access.
