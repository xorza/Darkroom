# Scenarium architecture and simplification review

## Executive summary

Scenarium has a clear compile → plan → resolve → execute pipeline. Since the
previous pass, the authoring model shed its largest duplication cluster: graph
normalization is gone, subgraph interfaces are authored state with reversible
detach/attach, library drift is tolerated at compile time instead of pruned,
and flattening keeps a resolved-graph stack instead of re-walking from the
root. The highest-impact remaining worker problem is unchanged: `send_many`
does not establish the batch boundary its callers rely on. The remaining
findings cluster around runtime state ownership (state retained by execution
id alone, advisory context declarations), per-run orchestration costs, one
leftover drift asymmetry (exposed events), and the detached-record pattern
that undo state duplicates.

## Current flow

`Compiler` validates an authored `Graph` (tolerating library-range drift in
bindings, subscriptions, and pins), recursively flattens composite instances
into an `ExecutionProgram` — dangling references degrade to unbound —
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

## High: Authoring and compilation invariants

- [ ] **Function registration accepts defaults that immediately create invalid
  nodes.** `FuncInput` carries an unconstrained `default_value` and
  `value_variants` (`src/node/definition.rs:65-68`, builder at `:100-103`);
  `Func::validate` checks neither (`src/node/definition.rs:339-383`) and
  `Library::add` treats that validation as the registration gate
  (`src/library.rs:130-131`). `Graph::add_func_node` and `add_graph_node`
  install the unchecked default as a `Const` binding
  (`src/graph/mod.rs:447-459`, `:462-478`), which `validate_for_execution`
  can then reject via `const_satisfies` (`src/graph/validate.rs:154-162`,
  `:310-334`) — e.g. any default on a `Custom`-typed input, or a default not
  among a nonempty `value_variants`. A valid registered function therefore
  does not imply its standard constructor produces a compilable node.

- [ ] **Exposed-event drift is the one remaining drift class that hard-fails
  compilation, with no repair path.** `validate_for_execution` still rejects
  `ExposedEventOutOfRange` when a `definition.events` entry references an
  interior emitter event index the current library no longer declares
  (`src/graph/validate.rs:196-206`, variant at `src/error.rs:90-91`), while
  flattening tolerates the identical dangling reference — `resolve_emitter`
  absorbs it via `.get(event_idx)?` and the Func-level drift check
  (`src/execution/flatten/mod.rs:322-327`, `:340-342`). Bindings,
  subscriptions, and pins all degrade to unbound under the same drift; a
  drifted exposed event instead makes the whole document uncompilable, and
  with normalization gone nothing ever removes it.

- [ ] **The advertised nesting cap neither exists in release builds nor
  protects the recursive validation that runs first.** Compilation validates
  the complete graph tree before flattening
  (`src/execution/compile.rs:129` → `:140`), and the recursive validator has
  no depth guard at all (`src/graph/validate.rs:34`, recursion at
  `:210-220` and `:225-244`; only shared-graph *cycles* are caught). The only
  depth check is a `debug_assert!` inside flatten's `push_level`
  (`src/execution/flatten/mod.rs:33`, `:159-162`). Deep acyclic input
  therefore has build-profile-dependent behavior and can exhaust the stack
  before or during compilation.

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

## Medium: Worker responsiveness and cancellation

- [ ] **`execute()` unconditionally resets the shared cancel token at run
  start, erasing a cancel requested before the run begins.**
  `src/worker/task.rs:224` clears the token before anything else, so a
  `Worker::request_cancel` (`src/worker/mod.rs:44-46`) landing between a
  batch drain and its run start is silently dropped and the run executes
  uncancelled. Latent today (darkroom only offers Cancel once `Executing` is
  reported, after the reset), but a real footgun for any caller that cancels
  a just-queued run.

- [ ] **The `biased` intent select can starve event-loop delivery under
  sustained host traffic.** `next_intent` polls `message_rx` ahead of the
  event branch (`src/worker/task.rs:135-148`); the event channel is bounded
  at 10 and event tasks block on `send().await`
  (`src/worker/event_loop.rs:13`, `:38`, `:63`). A continuous message stream
  keeps the message branch ready, so event ports are never drained and
  event-lambda progress stalls until the host stream quiesces.

- [ ] **A graph replace while an event loop is active emits a transient `Idle`
  status mid-rebuild.** A `Rebuild` transition stops the loop, which
  unconditionally reports `Idle` (`src/worker/task.rs:165-168`, `:290-292`)
  immediately before `execute()` re-emits `Executing` (`:229`). The host
  treats activity as absolute and repaints per report, so an update during an
  active loop produces a one-frame idle flicker. Cosmetic.

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

- [ ] **The doc on `ExecutionNode::special` describes a node kind that does
  not exist.** `src/execution/program/mod.rs:78-81` speaks of a cache node's
  path-keyed load/store, input pruning, and a bypass toggle riding in the
  variant — but `SpecialNode` has exactly one variant, `RunSinks`
  (`src/node/special.rs:20-27`). The comment actively misleads about current
  engine behavior.

- [ ] **Detached undo records duplicate the graph's ordered side-tables as
  manually validated public vectors — and the newer boundary records carry
  no validation at all.** `DetachedNode` re-represents the ordered
  map/set side-tables (`src/graph/mod.rs:257-271`) as public serializable
  vectors, manually re-derives their invariants, and converts back on attach
  (`src/graph/wiring.rs:44-101`, `:146-191`). `DetachedGraphInput`/`Output`
  follow the same pattern (`src/graph/boundary/mod.rs:18-44`) but their
  attach paths check only insert-overlap and `idx <= len`
  (`src/graph/boundary/mod.rs:112-151`, `:219-258`) — nothing verifies the
  recorded wiring actually references the detached slot, and re-added pins
  are silently absorbed where bindings assert
  (`src/graph/boundary/mod.rs:149`, `:239`). A malformed or hand-built
  record corrupts the owning graph's wiring without a guard.

## Open questions

- [ ] **Is `const_satisfies`' scalar coercion intentionally this loose?** An
  `Int`-typed port accepts a `Bool` or `Float` literal and vice versa
  (`src/graph/validate.rs:316-320`), mirroring
  `DataType::compatible_with`'s numeric coercion. It looks deliberate, but it
  means the const check cannot catch an authored `Bool` default on an `Int`
  config port, which reads as a stricter check than it is.
