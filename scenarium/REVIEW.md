# Scenarium architecture and simplification review

## Executive summary

Scenarium has a clear compile → plan → resolve → execute pipeline. Since the
previous pass, the authoring model shed its largest duplication cluster: graph
normalization is gone, subgraph interfaces are authored state with reversible
detach/attach, library drift *and type mismatches* are tolerated uniformly at
compile time (degrading to unbound at flatten) instead of pruned or severed,
registration gates declared defaults, deep nesting is a validation
error, and flattening keeps a resolved-graph stack instead of re-walking from
the root. The highest-impact remaining problem is unchanged: `Worker::send_many`
does not establish the batch boundary its callers rely on. The other open
findings cluster around runtime state ownership (state retained by execution
id alone, advisory context declarations), per-run orchestration costs, and
the parallel representations (`SpecialNode` dispatch, detached-record
vectors).

## Current flow

`Compiler` validates an authored `Graph` (tolerating library-range drift in
bindings, subscriptions, and pins), recursively flattens composite instances
into an `ExecutionProgram` — dangling references and type-mismatched
bindings degrade to unbound —
resolves output types, and returns a `CompiledGraph`. `WorkerTask`
opportunistically reduces ready messages and events into a `BatchIntent`,
installs compiled state, plans roots, prepares filesystem stamps, resolves
cache-aware liveness, executes surviving nodes, and publishes progress and
completion snapshots. `RuntimeCache` keeps output snapshots, function state,
event state, digests, and the disk store across runs and reconciles them by
flattened execution identity when a new program is installed.

## Resolved since the previous pass

- *Output normalization destroys authored output descriptions* — normalization
  was deleted; the interface is authored (`graph/boundary/`), renames touch
  only the name.
- *Normalization covers only part of the graph state validation treats as
  structural* — there is no normalize/prune pass to disagree with validation,
  and `validate_for_execution` now tolerates binding/subscription/pin range
  drift. The exposed-event remnant of this item survives as its own finding
  below.
- *Composite-interface validity depends on an unenforced boundary-node
  convention* — recharacterized as design: the interface is authored, boundary
  nodes are optional, and both validation and flattening treat a port without
  an interior counterpart as unbound (`graph/validate.rs:45-108`,
  `execution/flatten/mod.rs:407-427`).
- *Flattening repeatedly reconstructs the current graph from the root* — the
  per-build `Run` now keeps a `levels: Vec<&Graph>` stack parallel to `path`;
  the current graph is one stack read (`execution/flatten/mod.rs:126-171`).
- *Exposed-event drift hard-failed compilation* — the last drift class fell
  in line: `ExposedEventOutOfRange` was removed from `validate_for_execution`
  (flatten already wired the dangling event as nothing).
- *`execute()` erased a cancel requested before the run began* — the token
  now resets at the batch drain (`worker/task.rs`, `next_intent`), so a
  cancel raised after commit targets the imminent run.
- *A graph replace mid-event-loop flashed a transient `Idle` status* —
  intent application now stops the loop quietly; `Idle` is reported only
  when no run follows (terminal stops and panics still report it, in order).
- *`ExecutionNode::special`'s doc described a nonexistent cache node* —
  rewritten to the `RunSinks` reality.
- *`const_satisfies` rejected `Null` consts the runtime understands* —
  `Null` is now valid on optional inputs ("explicitly unset", matching
  lens's `Option`-field config reads) and still rejected on required ones.
- *`DetachedGraphInput`/`Output` attach accepted malformed records* — attach
  now panics unless every recorded binding and pin references the detached
  slot, and re-added pins assert like bindings do
  (`graph/boundary/mod.rs`, attach fns).
- *Open question: is the scalar-literal coercion intentionally loose?* —
  answered yes: it exactly mirrors the runtime `as_*` accessors and is now
  documented on `DataType::compatible_with`; declared defaults are the one
  place held to exact kinds (`Func::validate`'s `default_fits`).
- *A registered `Enum` default could name an unregistered variant* — the
  membership gate now runs from both registration directions (`Library::add`
  checks against present types; `register_type` re-checks the funcs already
  added), so declaration order doesn't matter.
- *The nesting cap was debug-only and validation recursed unguarded* —
  `validate_graph` now rejects trees past `MAX_NESTING_DEPTH` as a proper
  `NestingTooDeep` error before any deep recursion, and flatten's descent
  backstop is a release `assert!` (compile is cold; validation's
  shared-graph memoization can under-count true instance depth).

## High: Worker lifecycle

- [ ] **`Worker::send_many` does not create the worker commit boundary its API
  consumers rely on.** It is a plain loop of independent sends
  (`src/worker/mod.rs:52-60`), while the worker's atomic unit is whatever one
  `recv_many` wake happens to drain (`src/worker/task.rs:138`,
  `src/worker/task.rs:158-159`, `src/worker/batch.rs:40-72`). A wake on the
  first message can commit it before the rest of the burst is enqueued.
  Darkroom sends `Update` + `EvictCache` + `StopEventLoop` as one intended
  commit (`../darkroom/src/core/worker.rs:68-76`); if `Update` lands alone
  while an event loop is active, the transition is `Rebuild`
  (`src/worker/task.rs:36`), which re-runs and restarts the event loop —
  repopulating exactly the cache entries the not-yet-arrived eviction and
  stop were meant to protect.

## Medium: Cross-run state ownership

- [ ] **Program installation preserves function and event state solely because
  an execution ID still exists.** A runtime slot combines outputs with
  `AnyState` and `SharedAnyState` (`src/execution/cache/slot.rs:56-67`);
  installation replaces the program and `reconcile` retains the entire slot
  for every matching ID (`src/execution/engine/mod.rs:83-91`,
  `src/execution/cache/runtime/mod.rs:118-125`) without comparing function
  identity, version, or signature. Execution IDs encode only authoring UUIDs
  (`src/execution/identity.rs:21-38`), so a changed function implementation
  inherits state owned by the previous implementation even though output
  digests correctly invalidate its values.

- [ ] **Context identity and payload type are independent runtime choices.**
  `ContextType::new<T>` erases `T` (`src/runtime/context.rs:106-118`),
  `ContextManager::get<T>` accepts an unrelated requested type and panics on
  downcast (`src/runtime/context.rs:121-138`, panic at `:137`), and equality
  and hashing consider only the UUID (`src/runtime/context.rs:91-103`).
  Separately declared handles with one ID alias regardless of constructor or
  payload type, making ordinary context access depend on manually keeping
  three independent facts consistent.

- [ ] **Context declarations are advisory while `ContextManager` mixes
  persistent and per-run lifetimes.** `Func::required_contexts` has
  production writers but no production reader
  (`src/node/definition.rs:236`, `:328-331`; writers in lens, zero reads
  workspace-wide), and `ContextType::description` is only ever assigned
  `String::new()` (`src/runtime/context.rs:19`, `:114`). The manager
  simultaneously owns persistent resources, current-node attribution, logs,
  and cancellation (`src/runtime/context.rs:23-37`), and custom codecs
  receive that entire object merely to access resources during encoding
  (`src/execution/codec.rs:19-24`,
  `src/execution/disk_store/format/mod.rs:56-62`, `:96-99`).

- [ ] **The codec ABI is asymmetric: `encode` receives the `ContextManager`,
  `decode` receives nothing.** `CustomValueCodec::encode` takes
  `&mut ContextManager` while `decode` gets only the reader and byte length
  (`src/execution/codec.rs:19-31`). A custom type that needs a persistent
  runtime resource (a device, an allocator held in the manager's store) to
  reconstruct a value on read cannot obtain one, even though the same
  resource is available on write.

- [ ] **`DiskStore` retains the entire `Library` for codec lookup.** The store
  owns an `Arc<Library>` (`src/execution/disk_store/mod.rs:24-30`, set at
  `:60-67`) and every use passes it to format calls (`:110`, `:139`, `:200`)
  that call nothing but `library.codec(&type_id)`
  (`src/execution/disk_store/format/mod.rs:93`, `:176`, `:240`, `:304`).
  Cache I/O consequently retains unrelated functions, shared graphs, and
  editor-facing type metadata, tying cache-store replacement to changes in
  the broader registry.

## Medium: Per-run orchestration complexity

- [ ] **Targeted runs still rebuild full-program state across four node hash
  maps and two output columns.** Planning, DFS coloring, resolution, and
  execution outcomes each clear and repopulate state from every installed
  node before walking the reachable schedule
  (`src/execution/plan/mod.rs:92-105`, `:134-144`,
  `src/execution/resolve/mod.rs:106-117`,
  `src/execution/executor/mod.rs:114-121`), and resolution initializes every
  output entry (`src/execution/resolve/mod.rs:61-64`). A one-node preview
  therefore pays whole-graph hashing and initialization costs while the same
  identities and lifecycle states are duplicated across pipeline phases.

- [ ] **Resolution serially hydrates every reusable disk frontier before
  execution starts.** The reverse sweep awaits `check_reuse` per live node
  (`src/execution/resolve/mod.rs:131-140`, `:191-193`), and a disk hit
  immediately installs the decoded demand-scoped snapshot as resident
  (`src/execution/cache/runtime/mod.rs:246-253`). Independent disk reads
  accumulate into startup latency, and all accepted snapshots can occupy RAM
  together before the first lambda runs.

- [ ] **Live reporting adds an unbounded same-task relay followed by
  copy-on-write status snapshots.** Each run creates an unbounded channel
  polled in a `biased` select beside the engine future
  (`src/worker/task.rs:331`, `:335-343`) while the executor synchronously
  queues progress and pinned payloads
  (`src/execution/executor/mod.rs:248-255`, `:330-338`,
  `src/execution/executor/value_flow.rs:66-87`). A ready-heavy run completes
  before the recv branch is ever polled, so the whole event stream buffers
  and flushes only in the post-loop drain (`src/worker/task.rs:345-347`) —
  "live" updates arrive after completion. If an earlier published report
  remains queued, `Arc::make_mut` deep-clones the status vectors before the
  next update (`src/worker/status.rs:79`, `:188-192`).

- [ ] **The lambda ABI and executor call chain are wide, positional, and
  wrapper-heavy.** Every function receives six ordered borrows, including a
  mutable slice of one-field `InvokeInput` wrappers
  (`src/node/lambda.rs:67-70`, `:74-102`, `:125-150`), the macro duplicates
  four patterns around that exact order (`src/node/macros.rs:1-25`), and
  `Executor::run` suppresses its eight-argument warning
  (`src/execution/executor/mod.rs:95-106`). Changes to invocation state
  propagate through the public ABI, macros, executor, and every registered
  lambda.

## Medium: Worker responsiveness

- [ ] **The `biased` intent select can starve event-loop delivery under
  sustained host traffic.** `next_intent` polls `message_rx` ahead of the
  event branch (`src/worker/task.rs:135-148`); the event channel is bounded
  at 10 and event tasks block on `send().await`
  (`src/worker/event_loop.rs:13`, `:38`, `:63`). A continuous message stream
  keeps the message branch ready, so event ports are never drained and
  event-lambda progress stalls until the host stream quiesces.

## Medium: Parallel authoring representations

- [ ] **The single special node creates a parallel dispatch path throughout
  the ordinary function model.** `SpecialNode` contains only `RunSinks` and
  immediately maps back to a normal `Func` (`src/node/special.rs:20-27`,
  `:32-39`), yet `NodeKind` gives it a distinct serialized variant
  (`src/graph/mod.rs:139-150`) and every query, flattening, planning,
  program, and program-validation path carries special-node branches
  (`src/graph/query.rs:34`, `:47`, `:63`, `:119`,
  `src/execution/flatten/mod.rs:198`, `:319`, `:367`, `:401`,
  `src/execution/plan/mod.rs:268-283`, `src/execution/program/mod.rs:82`,
  `src/execution/validate.rs:100`). One planner-specific behavior expands
  the common authoring and execution state space across the crate.

- [ ] **Detached undo records duplicate the graph's ordered side-tables as
  manually validated public vectors.** `DetachedNode` re-represents the
  ordered map/set side-tables (`src/graph/mod.rs:257-271`) as public
  serializable vectors, manually re-derives their invariants, and converts
  back on attach (`src/graph/wiring.rs:44-101`, `:146-191`);
  `DetachedGraphInput`/`Output` follow the same pattern
  (`src/graph/boundary/mod.rs:18-44`, attach-time slot asserts). The second
  serializable representation still admits malformed states that only the
  attach-time panics reject, and every new detached kind re-derives the
  invariants the canonical containers already encode.
