# Design review: scenarium/src/worker/  (2026-04-23)

## Current design

`Worker` wraps a tokio task that owns an `ExecutionGraph` plus an optional running event-loop handle. The wire format is `Vec<WorkerMessage>` — one `send_many` is one atomic commit unit, and the worker opportunistically drains the unbounded command channel on wake (`mod.rs:285`) so multiple queued batches fold into a single commit. The task is a straight scan-then-commit pipeline: `scan()` is pure and folds an arbitrary batch into a `BatchIntent` with documented per-slot reduction rules (`mod.rs:199–207`, table at `mod.rs:190–198`); the commit phase runs the intent against live state in a fixed order (stop loop → graph op → execute → start loop → argument replies → syncs). Two inputs feed the intent: the command batch itself and the bounded lambda-event channel (`EVENT_LOOP_BACKPRESSURE = 10`); `biased` select prioritises commands so Stop/Exit can't be starved by a hot lambda.

Load-bearing decisions worth naming:

- **`Exit` dominates the whole batch** — not just "from here on". Every other command in the same batch is discarded, including pre-Exit ones. Oneshots close silently; waiters observe "sender dropped" (`mod.rs:223–233`).
- **`Update` forces a loop restart**: any batch touching `graph_state` or `loop_request` sets `needs_stop`, which unconditionally tears the loop down; the pre-stop running flag is then carried into `should_start_event_loop` so an Update-without-Start resumes (`mod.rs:311–338`).
- **The pause gate wraps only `execute()`**, not the whole commit phase, so lambdas pause exactly for the duration of a consistent-snapshot graph walk (`mod.rs:352`).
- **Stale-event filtering is structural**: `stop_event_loop` drops the `Receiver` alongside the `EventLoopHandle`, so any in-flight events die with the old channel — no generation counter needed (`mod.rs:395–404`).
- **`event_loop_started: Arc<AtomicBool>`** is a mirror of `event_loop.is_some()` exported for external polling from the UI thread (`session.rs:221`).

## Overall take

The core approach is right. Scan-then-commit with a pure `scan()` is the cleanest shape for "fold a batch of commands into atomic intent," and the per-slot reduction table is the right level of documentation. The submodule is tight, the invariants are stated in comments, and the non-trivial ones (Exit-dominates, drain-on-wake, structural stale-event drop, pause-gate-around-execute-only) each have at least one named test. The findings below are nudges, not a rethink. The test suite is the weaker side: good on semantics, thinner on behaviour during liveness (requests/syncs interleaved with a running loop, shutdown edges, one structurally weak assertion).

## Findings

### [F1] `Worker::send` panics if the worker task has exited

- **Category**: Contract
- **Impact**: 2/5 — minor sharp edge, but the one site racing it (`session.rs:342`) has no obvious reproducer today
- **Effort**: 2/5 — signature change on two methods plus five call sites
- **Current**: `send`/`send_many` `.expect("Failed to send message to worker, it probably exited")` (`mod.rs:106, 114`). `Worker::exit` also `.ok()`-swallows the same error (`mod.rs:119`), so the module already treats "worker gone" as a non-error in one place and a panic in another.
- **Problem**: Inconsistent handling of the same condition. A post-exit send crashes the host app; a post-exit exit silently succeeds. External callers have no way to check liveness before sending. The panic message is also misleading: the task can also be gone because it was aborted (`exit()` does both) or because it panicked.
- **Alternative**: Return `Result<(), WorkerExited>` from `send`/`send_many`. Treat the error as "harmless no-op" in the one caller (`session.rs`) the same way `exit()` already does.
- **Recommendation**: Do it. Cheap, removes a potential crash-on-race, unifies the two code paths.

### [F2] `event_loop_started: Arc<AtomicBool>` mirrors `event_loop.is_some()`

- **Category**: State
- **Impact**: 2/5 — one duplicated write each loop iter, two write sites (`mod.rs:269, 381`), no real bug but a "state that shouldn't exist" smell
- **Effort**: 3/5 — touches the public API (`is_event_loop_started`) and its one caller in `session.rs:221`
- **Current**: The `Option<(EventLoopHandle, Receiver)>` is the truth. The `AtomicBool` is a mirror kept in sync by two explicit stores per iteration — one at the top of the loop and one "refresh so racing Sync callers see post-commit state" at the bottom.
- **Problem**: Two writes-per-iter to maintain an observable that is trivially derivable. The second store exists only because the first is in the wrong place (it runs *before* commit, then goes stale until the *next* iteration's top). Any future commit-phase side effect that touches `event_loop` would need a matching store.
- **Alternative(s)**:
  1. **`tokio::sync::watch::<bool>`** — single producer (worker), many observers; eliminates the store-twice dance because the commit phase sends on the watch exactly when `event_loop` changes. Cost: a watch channel instead of an AtomicBool. Gives session.rs a way to *await* state changes instead of polling.
  2. **Query message**: `WorkerMessage::IsEventLoopStarted { reply }`. Loses synchrony with rendering (session polls every frame); rejected.
- **Recommendation**: Do option 1 when something else needs this signal; leave as-is otherwise. The current code is not buggy, just slightly load-bearing-by-convention.

### [F3] `stopped_event_loop_channel_is_closed` can hang on regression

- **Category**: Contract (test quality)
- **Impact**: 3/5 — a regression that *stops* closing the channel would hang CI rather than fail cleanly, hiding the actual failure
- **Effort**: 1/5 — rewrite the drain loop with a per-recv timeout or assert `.is_closed()` directly
- **Current**: `tests.rs:613–624`. The outer `timeout(500ms)` only wraps the *first* `recv()`. If the first call returns `Some` (buffered event), the inner `while event_rx.recv().await.is_some() {}` is unbounded. A regression where lambdas keep being re-spawned or the channel never closes would wedge the test past the outer timeout.
- **Problem**: The structural guarantee the test claims to verify (channel closes because all senders are aborted) is not actually asserted — the test currently passes even if `recv()` returns `Some` forever, up until the runtime kills it.
- **Alternative**: After `handle.stop().await`, assert closure directly: `assert!(event_rx.is_closed() || matches!(timeout(short, event_rx.recv()).await, Ok(None)))` — or a bounded drain loop using `timeout` on each iter.
- **Recommendation**: Do it. The test is the only pin on a subtle structural invariant; it should fail fast when broken.

### [F4] Missing: Exit in same batch as a Sync / RequestArgumentValues

- **Category**: Contract (test coverage)
- **Impact**: 3/5 — `scan()`'s "Exit dominates the batch, oneshots close silently, waiters get 'sender dropped'" is a stated contract (`mod.rs:222–223`) with no end-to-end test. If a future refactor of the scan → commit boundary accidentally reply-sends before draining, callers would start seeing stale `Ok(())` / `Ok(None)` instead of closed receivers.
- **Effort**: 1/5 — two short integration tests
- **Current**: `scan_exit_dominates_entire_batch` (`tests.rs:1148`) covers the pure-scan side; no runtime test confirms the oneshots close.
- **Alternative**: Add `exit_in_batch_closes_pending_sync` and `exit_in_batch_closes_pending_argument_request` that send `[Update, Sync { reply }, Exit]` / `[Update, RequestArgumentValues { reply }, Exit]` and assert the oneshot rx observes `Err(RecvError)`.
- **Recommendation**: Do it.

### [F5] Missing: `RequestArgumentValues` while the event loop is running

- **Category**: Contract (test coverage)
- **Impact**: 3/5 — this is the live-UI path (session pops an argument request every frame while `autorun` is on; `session.rs:324–337`). Covered by none of the existing tests — `request_argument_values_invokes_callback` (`tests.rs:453`) issues the request *after* execution, with no running loop.
- **Effort**: 1/5 — one test
- **Current**: No test exercises the commit-phase invariant "argument replies run post-execute, post-restart, so they see the freshest state even when a loop is running."
- **Alternative**: Add a test that `StartEventLoop`s, waits for a tick, then `RequestArgumentValues` and asserts `Some` with a value that reflects the latest execution (e.g. the `frame_no` input). This also pins that the pause-gate path doesn't deadlock the argument reply.
- **Recommendation**: Do it.

### [F6] Missing: StartEventLoop while already running

- **Category**: Contract (test coverage)
- **Impact**: 2/5 — `assert!(event_loop.is_none())` at `mod.rs:360` is a load-bearing invariant. It's only safe because `needs_stop` unconditionally tears the loop down before the start branch fires. A refactor that changes `needs_stop` conditions (e.g. "don't stop on Start if already running, it's wasteful") would hit the assert.
- **Effort**: 1/5 — one test
- **Current**: None.
- **Alternative**: `start_event_loop_twice_is_idempotent` — send Start, wait, send Start again, assert no panic and loop still running.
- **Recommendation**: Do it.

### [F7] Missing: `is_event_loop_started()` observable after post-commit refresh

- **Category**: Contract (test coverage)
- **Impact**: 2/5 — `mod.rs:381` explicitly re-stores the atomic "so callers racing a Sync reply see the post-commit state." No test pins this ordering; if it regresses, `session.rs:218` starts lagging one frame behind on autorun-toggle.
- **Effort**: 1/5 — one test
- **Current**: `start_stop_event_loop` (`tests.rs:413`) checks the atomic after an `await sleep`, which could mask an ordering bug.
- **Alternative**: Batch `[StartEventLoop, Sync { reply }]`, wait on the Sync, then **immediately** assert `is_event_loop_started() == true` without any further sleep/yield. Same shape for StopEventLoop → Sync → `== false`.
- **Recommendation**: Do it. Cheap, directly pins the invariant the comment describes.

### [F8] Missing: Drop-without-explicit-exit

- **Category**: Contract (test coverage)
- **Impact**: 2/5 — `Drop` calls `exit()` (`mod.rs:128`). Every existing test calls `exit()` explicitly; Drop's cleanup path is never exercised in isolation. If someone removes the Drop impl or breaks `exit()`'s idempotency, tests pass.
- **Effort**: 1/5 — one test
- **Current**: None.
- **Alternative**: `drop_without_exit_shuts_down_cleanly` — construct a worker in a scope, drop it, run another short tokio op on the same runtime and confirm no panic/hang. Optionally: hold a clone of the `event_loop_started` `Arc` across the drop and assert it stays observable (no poison).
- **Recommendation**: Do it.

### [F9] Test fixture boilerplate repeats 6× across tests

- **Category**: Responsibility (test quality)
- **Impact**: 1/5 — cosmetic, but the setup takes ~10 lines per test and is duplicated in `test_worker`, `clear_resets_execution_graph`, `events_are_deduplicated`, `start_stop_event_loop`, `request_argument_values_invokes_callback`, `sync_fires_after_execution`, `update_restarts_event_loop_if_running`, `clear_then_update_in_same_batch_applies_update`, `commands_not_starved_by_fast_event_loop`, `lambda_events_drive_worker_execution`, `update_then_clear_in_same_batch_leaves_graph_cleared`, `request_argument_values_batched_with_update_sees_new_graph`, `request_argument_values_batched_with_clear_sees_empty_graph`. That's 13 near-identical blocks of `WorkerEventsFuncLib` + `BasicFuncLib` + merge + `log_frame_no_graph` + channel wiring.
- **Effort**: 1/5 — extract one helper
- **Current**: Each test copies the block. Adding a new basic-funclib-dependent test means copy-paste.
- **Alternative**: `fn frame_harness() -> (Worker, mpsc::Receiver<Result<ExecutionStats>>, OutputStream, NodeId /* frame_event */, Arc<FuncLib>, Graph)` or similar. Tests that don't need `OutputStream` use `BasicFuncLib::default()` via a second helper. The tests that already diverge (terminal-only graphs, empty-worker tests) don't use it.
- **Recommendation**: Do it. Drops ~80 lines and makes the "what's this test actually testing" signal louder.

### [F10] `EVENT_LOOP_BACKPRESSURE` conflates channel capacity with batch size

- **Category**: Types
- **Impact**: 1/5 — one constant used for two things that happen to coincide today
- **Effort**: 1/5 — split into two named constants if anyone ever wants to tune them separately
- **Current**: `mod.rs:23`. Used as channel capacity at `mod.rs:416` and as `recv_many` limit at `mod.rs:291`.
- **Problem**: The natural question "could we buffer more events than we drain per wakeup?" currently reads as "no, by definition," but only because one constant wears both hats.
- **Alternative**: Leave alone until you actually want to tune one independently. Mentioning here because the comment at `mod.rs:20–23` describes one of the two roles (capacity) and not the other (per-wake batch).
- **Recommendation**: Don't do it. File the finding mentally; revisit if you ever tune backpressure.

## Considered and rejected

- **Collapse `scan` into the main loop.** `scan` is pure, unit-testable, and the batch-reduction logic is exactly the part most worth isolating. Keep it.
- **Replace `BatchIntent` fields with a state-machine enum.** The fields are orthogonal (graph op, loop request, execute flag, events, replies) and the per-slot reductions are genuinely per-slot. An enum would force a cross product.
- **Drop the `Arc<AtomicBool>` and expose a method that blocks on a message roundtrip.** Session polls this on every frame's `update_shared_status`; a roundtrip-per-frame is the wrong trade.
- **Add a generation counter for stale-event filtering.** The structural approach (drop the Receiver) already gives the guarantee without extra state; the existing code is better.
