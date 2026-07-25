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
findings cluster around per-run orchestration costs and the parallel
representations (`SpecialNode` dispatch, detached-record vectors).

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
  drift (while gaining two structural rejections: `EntryBoundaryNodes` and
  `ConstOnlyBinding`). The exposed-event remnant of this item survives as its
  own finding below.
- *Composite-interface validity depends on an unenforced boundary-node
  convention* — recharacterized as design: the interface is authored, boundary
  nodes are optional, and both validation and flattening treat a port without
  an interior counterpart as unbound (`graph/validate.rs:133-186`,
  `execution/flatten/mod.rs:415-438`).
- *Flattening repeatedly reconstructs the current graph from the root* — the
  per-build `Run` now keeps a `levels: Vec<&Graph>` stack parallel to `path`;
  the current graph is one stack read (`execution/flatten/mod.rs:124-152`).
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
  slot (`assert_targets_slot`, `graph/boundary/mod.rs:60-122`, run before any
  mutation), and re-added pins assert like bindings do.
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
- *Program installation preserved function and event state solely because an
  execution ID still existed* — each `RuntimeSlot` now records a `StateOwner`
  (func id + version, the same identity the digest folds); `reconcile` drops
  `state`/`event_state` when the installed node's owner differs and leaves
  the digest-keyed value alone, and `validate_installed` asserts the pairing
  (`src/execution/cache/slot.rs`, `reown`).
- *Context identity and payload type were independent runtime choices* — the
  UUID is gone: `ContextType<T>` is a typed `Copy` handle whose payload type
  is the identity, the store is keyed by `TypeId::of::<T>()`, and `get`
  cannot request a type its handle doesn't declare (`src/runtime/context.rs`).
  The advisory remnants fell with it: `Func::required_contexts` (writers, no
  readers) and the always-empty `ContextType::description` were deleted, and
  lens declares `VISION_CTX_TYPE` as a plain `const`.
- *"Live" progress flushed only after a ready-heavy run completed* — the run
  loop now yields once per scheduled node (`task::yield_now` at the loop
  top, `src/execution/executor/mod.rs`), so the `biased` select drains per
  node and a node's `Started`/`Finished` reach the host before the next
  lambda runs.
- *The codec ABI was asymmetric and over-broad, and `ContextManager` mixed
  persistent and per-run lifetimes for its consumers* — the persistent
  resource half is now its own `ContextStore` (`src/runtime/context.rs`),
  and both `CustomValueCodec::encode` **and** `decode` receive
  `&mut ContextStore` — nothing else — threaded through resolve's disk
  hydration (`Resolver::resolve` → `check_reuse` → `DiskStore::read`), so a
  codec can reconstruct resource-backed values on read. Lambdas still get
  the full `ContextManager` (contexts + logs + cancel), which is its
  purpose.
- *`DiskStore` retained the entire `Library` for codec lookup* — the store
  now owns only a `Codecs` map (`TypeId → Arc<dyn CustomValueCodec>`,
  `src/execution/codec.rs`) extracted from the library at construction;
  format calls take `&Codecs`, and cache I/O no longer holds funcs, shared
  graphs, or editor metadata.
- *`RuntimeCache::slots` was the last id-hashed per-node structure in the run
  loop* — slots are now a `NodeColumn<RuntimeSlot>` aligned to the installed
  program; `reconcile` re-pairs the previous install's slots with the new
  index order by stable id (the only id hashing slot access ever pays), and
  every executor/resolver/digest slot access is an index read. Disk blobs
  stay id-named so they survive installs that shift indices.
- *Targeted runs rebuilt full-program state across four node hash maps* —
  the installed program is now a dense, id-sorted vector with `NodeIdx`
  positions; compiled `Bind` edges intern to `OutputAddr` at compile
  (`ExecutionProgram::intern_bindings`), and plan verdicts, DFS colors,
  dispositions, and outcomes are `NodeIdx`-aligned columns and bitsets whose
  resets are memsets. Per-run and per-edge id hashing is gone from planning,
  resolution, and execution; ids are hashed only at the host boundary (seed
  resolution, slot access, report emission). `NodeIdx` is install-local and
  never enters digests, persisted bytes, or reports.

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

## Medium: Per-run orchestration complexity

- [ ] **Resolution serially hydrates every reusable disk frontier before
  execution starts.** The reverse sweep awaits `check_reuse` per live node
  (`src/execution/resolve/mod.rs:176-204`, await at `:191-193`), and a disk hit
  immediately installs the decoded demand-scoped snapshot as resident
  (`src/execution/cache/runtime/mod.rs:246-253`). Independent disk reads
  accumulate into startup latency, and all accepted snapshots can occupy RAM
  together before the first lambda runs.

- [ ] **Live reporting relays through an unbounded same-task channel into
  copy-on-write status snapshots.** Each run creates an unbounded channel
  polled in a `biased` select beside the engine future
  (`src/worker/task.rs:352`, `:356-364`) while the executor synchronously
  queues progress and pinned payloads
  (`src/execution/executor/mod.rs:248-255`, `:330-338`,
  `src/execution/executor/value_flow.rs`). If an earlier published report
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
  event-lambda progress stalls until the host stream quiesces. The bias is
  intentional and test-locked in the opposite direction
  (`commands_not_starved_by_fast_event_loop`, `src/worker/tests.rs:1863-1869`),
  so any fix must preserve bounded-latency command observation rather than
  simply removing `biased`.

## Medium: Parallel authoring representations

- [ ] **The single special node creates a parallel dispatch path throughout
  the ordinary function model.** `SpecialNode` contains only `RunSinks` and
  immediately maps back to a normal `Func` (`src/node/special.rs:20-27`,
  `:32-39`), yet `NodeKind` gives it a distinct serialized variant
  (`src/graph/mod.rs:141-151`). The `NodePorts` unification collapsed the
  four per-kind query matches into one branch (`src/graph/query.rs:124`),
  but flattening, planning, the program, and program validation still carry
  special-node branches (`src/execution/flatten/mod.rs:199`, `:320`, `:363`,
  `:397`, `src/execution/plan/mod.rs:268-283`,
  `src/execution/program/mod.rs:82`, `:134`,
  `src/execution/validate.rs:100`), and authoring validation keeps a no-op
  arm just to satisfy the match (`src/graph/validate.rs:113`). One
  planner-specific behavior expands the common authoring and execution state
  space across the crate.

- [ ] **Detached undo records duplicate the graph's ordered side-tables as
  manually validated public vectors.** `DetachedNode` re-represents the
  ordered map/set side-tables (`src/graph/mod.rs:220-234`) as public
  serializable vectors, manually re-derives their invariants (`assert_valid`
  hand-asserts sortedness, `src/graph/wiring.rs:44-98`), and converts back
  on attach (`src/graph/wiring.rs:142-185`); `DetachedGraphInput`/`Output`
  follow the same pattern (`src/graph/boundary/mod.rs:21-51`,
  `assert_targets_slot` at `:60-122`). The boundary refactor moved the
  per-record asserts ahead of any mutation, but the overlap checks still
  fire mid-mutation — `restore_bindings`/`restore_pins`
  (`src/graph/boundary/mod.rs:377-396`) panic after the instance slots have
  already shifted. The second serializable representation still admits
  malformed states that only attach-time panics reject, and every new
  detached kind re-derives the invariants the canonical containers already
  encode.
