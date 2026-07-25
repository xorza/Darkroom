# Scenarium architecture and simplification review

## Executive summary

Scenarium has a clear compile → plan → resolve → execute pipeline. Since the
previous pass, the authoring model shed its largest duplication cluster: graph
normalization is gone, subgraph interfaces are authored state with reversible
detach/attach, library drift _and type mismatches_ are tolerated uniformly at
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
