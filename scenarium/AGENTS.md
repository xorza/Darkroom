# Scenarium

Scenarium is the node-graph framework: a serializable authoring model, a
compile → plan → execute pipeline, and an asynchronous worker. It depends only
on `common` in-tree. `lib.rs` is the public façade; implementation modules are
crate-private, so downstream crates import public concepts directly from
`scenarium`.

## Models and identities

The authoring `Graph` owns identity-only `Node`s plus side tables for input
bindings, event subscriptions, pinned outputs, and local subgraph definitions.
It is the persisted model. `Graph::check` enforces node-id uniqueness across the
whole reachable authoring tree. Node removal and restoration use `DetachedNode`,
which keeps the node, all touching wiring, subscriptions, and pins together.

Compilation produces a private, immutable `ExecutionProgram`. Composite nodes
are dissolved into flat function nodes and SoA pools. Top-level nodes retain
their `NodeId`; nested flat ids are derived with domain-separated BLAKE3.
`FlattenMap` maps both directions between flat ids and exact `NodeAddress`
values. A `NodeAddress` contains the subgraph-instance path plus the definition
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
| `node/lambda.rs` | Function invocation ABI and output demand |
| `node/event.rs` | Event-lambda ABI |
| `graph/mod.rs` | Core authoring structs, construction, serialization, and validation |
| `graph/wiring.rs` | Wiring mutation, scoped node detach/attach, cycle checks |
| `graph/clone.rs` | Deep copies with fresh node ids |
| `graph/reconcile.rs` | Stale-library wiring cleanup |
| `graph/query.rs` | Type and reachability queries |
| `graph/subgraph.rs` | Composite definitions and references |
| `execution/compile.rs` | Host-side compiler and compiled artifact |
| `execution/flatten/` | Composite lowering |
| `execution/identity.rs` | Scoped authoring addresses and flatten provenance |
| `execution/program.rs` | Private flat runtime program |
| `execution/plan/` | Structural scheduling and output demand |
| `execution/resolve/` | Demand-aware cache reuse and cone pruning |
| `execution/executor/` | Invocation, delivery, reclamation, and stats |
| `execution/cache/` | Cross-run values and materialization masks |
| `execution/codec.rs` | Cache framing plus downstream custom-value codec API |
| `execution/disk_store/` | On-disk cache persistence |
| `execution/report.rs` | Live progress and pinned-output transport |
| `execution/stats.rs` | Completed-run results |
| `worker/protocol.rs` | Host/worker messages and reports |
| `worker/batch.rs` | Ordered batch reduction |
| `worker/event_loop.rs` | Active event-task lifecycle |
| `worker/mod.rs` | Worker handle and task orchestration |

## Compile, plan, execute

`Compiler::compile` runs synchronously on the host and returns a
`CompiledGraph`; compile errors never enter the worker. Planning is structural:
it selects roots, orders dependencies before consumers, detects missing inputs,
and computes immutable `OutputDemand` plus binding-reader counts for every
output. Execution resolves content digests and reuse, prunes cones that feed
only reusable results, then invokes surviving nodes in plan order.

A cache slot is valid only when its digest matches and its
`OutputSnapshot` coverage contains every currently demanded output. Invocation
clears the output buffer first, so an output the lambda skips cannot retain a
stale value. Disk frames persist the same coverage; a same-digest write replaces
an older frame when the new result covers more outputs.

Only `FuncBehavior::Pure` cones receive reusable content digests. Resource
inputs fold the current referent identity through `ResourceStamper`; filesystem
paths have built-in stamping. Custom runtime values are not serializable.
Downstream types attach a `CustomValueCodec` explicitly when they want disk
cache support.

## Worker

One `Vec<WorkerMessage>` is one commit unit. `BatchIntent` preserves first-seen
order while deduplicating node seeds and events; conflicting state slots are
last-write-wins and `Exit` dominates its batch. `ActiveEventLoop` owns both its
tasks and event receiver, so the lifecycle invariant is represented by one
type. Worker reports stream progress and exact scoped pinned outputs before the
matching finished result.

## Built-ins and tests

The filesystem watcher and random node are optional features
(`builtin-fs-watch`, `builtin-random`). Darkroom enables them explicitly. Test
fixtures and private-state builders are available only under tests or the
`internals` feature; downstream crates enable `internals` only as a dev
dependency. Test helpers stay in gated `test_support` modules beside the
private state they access.
