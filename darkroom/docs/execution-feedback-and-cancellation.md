# Realtime execution feedback + cancellation — design

> **Status: P1 (per-node progress) shipped.** The worker now streams a
> `WorkerReport::Progress(RunProgress)` per node ahead of the final
> `WorkerReport::Finished`; darkroom marks the active node `ExecStatus::Running`
> (purple glow) live. P2 (cancel) / P3 (cooperative lumos stop) are still open —
> see Phasing. Implementation notes are folded into the steps below.

Goal: while a long-running graph runs, the editor should show **which node is
computing right now** (and that it started), and the user should be able to
**cancel the run and actually stop** long operations (`stack_lights`,
`build_masters`). P1 delivers the first half; cancellation is still TODO.

Spans `scenarium` (the headless executor + worker) and `darkroom` (the editor
feedback + controls). `lumos` is touched only in the final phase (cooperative
stop inside the heavy ops).

---

## Current architecture (the seams this builds on)

**One report, at the end.** The worker takes a single callback
`Fn(Result<ExecutionStats>)` (`scenarium/src/worker/mod.rs:86`) and fires it
**once** when a run finishes. `ExecutionStats`
(`scenarium/src/execution_stats.rs`) is a monolithic post-run summary
(`executed_nodes`, `cached_nodes`, `missing_inputs`, `node_errors`, `logs`,
`flatten`). There is **no per-node "started" / "in progress" signal**.

**The executor is a sequential, inline-`await` loop.** `Executor::run`
(`scenarium/src/execution/executor.rs:91`) walks `plan.execute_order`, and for
each node `await`s `lambda.invoke(...)` inline (`executor.rs:142`). Heavy lumos
nodes offload to `tokio::task::spawn_blocking` *inside* the lambda, then `.await`
the join handle (e.g. `run_frame_op`, `stack_lights`, `build_masters` in
`lens/src/astro/funclib/mod.rs`).

**No cancellation anywhere on the run path.** Lambdas receive
`(&mut ContextManager, &mut AnyState, &SharedAnyState, &[InvokeInput],
&[OutputUsage], &mut [DynamicValue])` (`scenarium/src/func_lambda.rs`) — no
token. The only abort that exists is the **event-loop** teardown
(`EventLoopHandle::stop` → `JoinHandle::abort`, `worker/mod.rs:174`), which does
not touch a `execute()` run in flight.

**The worker loop processes one command batch, then awaits the whole run.** The
`tokio::select! { biased }` loop (`worker/mod.rs:322`) collapses a batch, then
`await`s `execution_engine.execute(...)` to completion (`worker/mod.rs:409`)
before looping. So a message arriving *during* a run isn't seen until the run
ends — a `Cancel` would be ignored until too late. **This is the central change
both features need.**

**darkroom side.** `App::run_graph` → `engine.run_once(graph)` → `WorkerBridge`
sends `[Update, ExecuteTerminals]` (`darkroom/src/core/worker.rs:110`). The
worker callback delivers `WorkerEvent::ExecutionFinished(stats)` over an mpsc +
wakes the UI (`worker.rs:101`). `App::drain_worker_events`
(`app/mod.rs:126`) drains it into `RunState::set_results`
(`gui/run_state.rs:115`), which attributes flat stats back to authoring nodes via
`stats.flatten`. `Scene::rebuild` copies `run_state.status(id)` onto
`SceneNode.exec_status` (`scene.rs:303`), and the node body glows via
`exec_shadow`/`exec_color` (`gui/node/mod.rs`). `ExecStatus`
(`run_state.rs:37`) = `None | Cached | Executed(f64) | MissingInputs | Errored`
— **no `Running`**.

---

## Feature A — realtime per-node progress

### Design

Turn the worker's single end-of-run callback into a **stream**: the executor
emits a progress event *before* and *after* each node, the worker forwards it,
darkroom marks that node `Running` (then clears it when the node finishes / the
run ends).

**1. A progress sink threaded into the executor.** Add a callback parameter to
the run path (a `&mut dyn FnMut(RunProgress)` — no allocation, no channel inside
scenarium):

```rust
// scenarium/src/execution_stats.rs  (new)
pub enum RunPhase { Started, Finished { elapsed_secs: f64 } }
pub struct RunProgress {
    /// Authoring node id(s) this flat node attributes to (resolved via the
    /// FlattenMap so the host needn't know the flattening).
    pub nodes: smallvec::SmallVec<[NodeId; 1]>,
    pub phase: RunPhase,
}
```

`Executor::run` (`executor.rs:108` loop) calls `progress(Started)` right before
`lambda.invoke(...).await` and `progress(Finished{elapsed})` right after,
resolving the flat id → authoring ids with the same attribution `set_results`
uses (`stats.flatten.attribution(flat_id)` — available during the run, it's
built at flatten/compile time). Cached/skipped nodes can emit nothing (they're
in the final stats) or a `Finished` immediately.

**2. The worker forwards progress as a report variant.** Replace the worker's
`Fn(Result<ExecutionStats>)` callback with a richer report enum (one callback,
contained change):

```rust
// scenarium/src/worker/mod.rs
pub enum WorkerReport {
    Progress(RunProgress),
    Finished(Result<ExecutionStats>),
}
// Worker::new<C: Fn(WorkerReport) + Send + 'static>(callback: C)
```

The worker passes a closure into `execute(...)` that wraps each `RunProgress` as
`WorkerReport::Progress` and calls the callback; the final stats become
`WorkerReport::Finished`. (Thread the sink through
`ExecutionEngine::execute` → `Executor::run`.)

**3. darkroom maps progress → a live `Running` status.**
- Add `ExecStatus::Running` (`run_state.rs:37`) and a `exec_running_glow` theme
  color (`exec_color`, `gui/node/mod.rs`) — a distinct pulsing/accent glow.
- Add `WorkerEvent::NodeProgress(RunProgress)` (`core/worker.rs:42`); the
  bridge's `deliver` routes `WorkerReport::Progress` → that, `Finished` → the
  existing `ExecutionFinished`.
- `drain_worker_events` (`app/mod.rs:126`): on `NodeProgress::Started` set those
  authoring nodes to `Running`; on `Finished` set `Executed(elapsed)` (or leave
  for the end-of-run `set_results` to finalize). `RunState::begin_run`
  (`run_state.rs:166`) already runs at launch — extend it to mark the
  terminal-reachable nodes `Running`/`Pending` so the user sees the run "start"
  immediately, before the first node reports.

**Frame latency:** each progress event wakes the UI (the existing `wake()` in
`deliver`), so "now computing X" updates within a frame of the node starting.
Because the executor is sequential, exactly one node is `Running` at a time —
simple and accurate.

### Steps (Feature A)
1. scenarium: `RunProgress`/`RunPhase` types; thread a `progress: &mut dyn
   FnMut(RunProgress)` through `execute` → `Executor::run`; emit Started/Finished
   per node (resolve flat→authoring via the flatten map). Unit-test that a
   2-node graph emits Started/Finished in order.
2. scenarium: `WorkerReport` enum; change `Worker::new` callback to
   `Fn(WorkerReport)`; the worker forwards progress + final stats. Update the one
   call site (darkroom).
3. darkroom: `ExecStatus::Running` + `exec_running_glow`; `WorkerEvent::NodeProgress`;
   route in `deliver` + `drain_worker_events`; mark nodes `Running` in/after
   `begin_run`. Test `RunState` transitions.
4. (polish) a small header/badge or canvas overlay showing the active node name +
   a spinner while a run is in flight.

---

## Feature B — cancellation

Two levels, shipped in order. **B1** makes the *editor* stop and become
responsive immediately (no lumos changes). **B2** makes the *CPU work* actually
stop promptly (requires cooperative checks in lumos).

### B1 — coarse cancel + responsive worker (no lumos changes)

The worker holds a per-run `CancellationToken` (tokio-util — already a workspace
dep; or `Arc<AtomicBool>` + `Notify`). A new `WorkerMessage::Cancel` trips it.

**Make the worker observe Cancel mid-run** (the load-bearing change). P1 already
built half of this: the worker now runs `execute()` as a pinned future inside a
`tokio::select!` loop that drains the progress channel concurrently (P1 chose an
`UnboundedSender<RunProgress>` over a borrowed `&mut dyn FnMut` precisely so the
host callback needn't be `Sync` and so this loop exists). B1 just adds a second
select arm on the command channel that trips the cancel token. Today the
remaining gap: outside that loop the worker still folds one command batch at a
time. Restructure so the in-flight run is raced against incoming commands too:

```rust
let run = self.engine.execute(terminals, events, token.clone(), &mut progress);
tokio::pin!(run);
loop {
    tokio::select! {
        stats = &mut run => break stats,                  // run finished/bailed
        Some(batch) = rx.recv() => {
            if batch.wants_cancel() { token.cancel(); }   // trip; run bails next
            else { /* stash for after this run */ }
        }
    }
}
```

**Executor honors the token between nodes.** `Executor::run` checks
`token.is_cancelled()` at the top of the `execute_order` loop
(`executor.rs:108`); if set, stop scheduling further nodes and return a stats
marked `cancelled`. The *in-flight* node still runs to completion (B1 limitation
— a stack already started keeps going on the blocking pool), but:
- no further nodes start, and
- the worker stops `await`ing it (the `select!` arm returns once the executor
  yields), so the editor is immediately responsive and the orphaned
  `spawn_blocking` result is dropped when it eventually lands.

**Report + UI.** Add `RunPhase::Cancelled` / a `WorkerReport::Cancelled` (or a
flag on the finished stats). darkroom: a **Cancel button** (menu + a
`Esc`/`Ctrl+.` shortcut) shown only while a run is in flight (`RunState` already
knows via `run_id`/a `running` flag); wire `MenuCommand::CancelRun` →
`Engine::cancel_run` → `WorkerBridge` sends `WorkerMessage::Cancel`. On a
cancelled report, clear `Running` glows.

### B2 — cooperative stop inside the heavy ops (lumos)

B1 won't interrupt a stack mid-computation (tokio can't kill a `spawn_blocking`
thread). To *truly* stop, the op must poll a cancel flag and bail.

**Plumb a cancel flag to lambdas.** Expose it on `ContextManager` (the per-run
resource store the lambda already gets): `ctx_manager.cancel_token()` →
a cheap-to-poll handle (`CancellationToken` exposes `is_cancelled()`; or hand out
an `Arc<AtomicBool>` derived from it — ideal for polling in a rayon loop). The
executor owns it for the run; the worker trips it on `Cancel`.

**Lambdas pass it into the `spawn_blocking` closure → into lumos.** e.g.
`stack_cfa_master(paths, config, &cancel)`, `calibrate_align_stack(..., &cancel)`,
`run_frame_op(value, cancel, op)`. lumos's chunked/rayon loops
(`stacking::combine` `process_chunks`, `calibration_masters::from_files`, the
registration/detection passes) check `cancel.is_cancelled()` between chunks/frames
and return a `Cancelled` error early. This is the only way the CPU work stops
promptly; it's a lumos API addition (a new `&CancelFlag` param + periodic checks
in the stage loops), so it lands last and incrementally (start with the two the
user named: stacking masters + light stacking).

The lambda maps a lumos `Cancelled` into an `InvokeError` (or a dedicated
`InvokeResult` "cancelled" so the executor can mark the node cancelled rather
than errored).

### Steps (Feature B)
1. scenarium: a `CancelToken` on the run (`CancellationToken` or
   `Arc<AtomicBool>`); `WorkerMessage::Cancel`; thread the token into
   `execute` → `Executor::run`; check `is_cancelled()` at the loop top.
2. scenarium: restructure the worker loop to `select!` the in-flight run against
   the command channel so `Cancel` is observed mid-run (stash other mid-run
   commands for after). Emit a cancelled report.
3. darkroom: Cancel button + shortcut, `MenuCommand::CancelRun`,
   `Engine::cancel_run` → `WorkerBridge` → `WorkerMessage::Cancel`; clear glows
   on cancelled report; show a "cancelling…" state until the worker confirms.
4. scenarium: expose `ContextManager::cancel_token()`; pass it to the
   `InvokeInput`/context the lambdas read.
5. lumos: add a `&CancelFlag` (or `&CancellationToken`) param to
   `stacking::combine`/`calibration_masters` (the two named ops first) +
   poll it in the chunk loops, returning `Cancelled` early. Then registration /
   detection / drizzle.
6. lens: thread the token from `ctx_manager` into the `spawn_blocking` calls in
   `stack_lights` / `build_masters` (then the per-frame ops); map `Cancelled`
   → a clean node outcome.

---

## Phasing (smallest shippable slices)

- **P1 — progress (Feature A, steps 1–3):** "now computing X" + run-in-flight
  indicator. No cancel yet. Self-contained scenarium+darkroom change.
- **P2 — coarse cancel (B1):** Cancel button stops scheduling + frees the editor.
  In-flight op still finishes in the background (its result discarded). Honest UX
  caveat to surface: "stopping…" until the current op drains.
- **P3 — cooperative stop (B2):** the named heavy ops (`stack_lights`,
  `build_masters`) actually bail early. Roll out per lumos stage.
- **P4 — polish:** active-node spinner/label, elapsed-so-far on the running node,
  per-node cancel (cancel just the hovered node's subtree) if wanted.

Each phase is independently useful; P1 alone already answers "which node is
computing now / did it start".

---

## Decisions / open questions

- **Token type:** `tokio_util::sync::CancellationToken` (await + `is_cancelled()`
  poll in one; workspace already pins `tokio-util`) vs. a bespoke
  `Arc<AtomicBool>` + `Notify`. Recommend `CancellationToken` for the worker/await
  side; it also polls cheaply for the lumos loops. Decide before P2.
- **Worker callback shape:** one `Fn(WorkerReport)` enum *(recommended)* vs. a
  second progress callback alongside the existing result callback. The enum is
  one call site to update and unifies delivery.
- **Cancelled vs errored outcome:** add a distinct `cancelled` node/run outcome
  (not `Errored`) so the UI + logs read correctly. Small `ExecutionStats` +
  `ExecStatus` addition.
- **Mid-run commands:** while a run is in flight and a non-Cancel batch arrives
  (e.g. an edit/`Update`), do we (a) cancel-then-apply, or (b) stash-and-apply
  after? Recommend: `Update`/`Clear`/`Exit` imply cancel (they already tear down
  the event loop); other commands stash. Confirm.
- **Cooperative granularity in lumos:** how often to poll the flag (per chunk /
  per frame / per N rows) — frequent enough to feel instant, rare enough to not
  cost throughput. Per-chunk in `combine`/`from_files` is the natural seam.
