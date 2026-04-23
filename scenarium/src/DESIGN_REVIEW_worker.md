# Design review: scenarium/src/worker.rs  (2026-04-23)

## Current design

`Worker` owns a tokio task running `worker_loop`. Commands flow in on an `UnboundedSender<WorkerMessage>`. An `Arc<AtomicBool>` mirrors "event loop running" so callers (`session.rs`) can poll without a round-trip.

The loop body is a two-phase scan/commit over batches. Each iteration wakes on a `biased` select between two channels: a command channel (`recv_many` up to `WORKER_MSG_BATCH=10`) and, when an event loop is running, a dedicated `Receiver<EventRef>` fed directly by spawned lambda tasks. `scan()` folds the batch (including nested `Multi` variants) into a `BatchIntent` — flags, an event `HashSet`, and reply senders. The commit phase applies `reset → clear → update → execute → maybe-start-loop → reply to arg-value requests → ack`, re-storing the atomic on either side of commit. A `PauseGate` closes around the commit so lambdas can't race state mutations.

Event loop itself is structural: `start_event_loop` spawns one task per `EventTrigger` looping `lambda → send → pause_gate.wait → yield`, and returns the `Receiver` directly. `reset_event_loop` aborts tasks and drops the receiver, so stale events die with the channel — no sequence numbers, no generation checks.

## Overall take

Core shape is right. Scan-then-commit with a dedicated `BatchIntent` is a sharp pattern that's already earned its keep (the recent Clear+Update+Request batching fixes would have been horrifying to implement against per-message handling). Biased-select + structural stale-event killing is clean.

The weaknesses are at the wire format and the external API surface, not in the loop itself: `Multi` and `WORKER_MSG_BATCH` together make "atomic batch" an emergent property rather than a guarantee; the commit phase's fixed order silently rewrites the semantics of Update-then-Clear; the `AtomicBool` is a legitimately-lagging state dressed up as a synchronous query.

## Findings

### [F1] `WORKER_MSG_BATCH` + `Multi` is the wrong batching primitive
- **Category**: Data structures / Contract
- **Impact**: 4/5 — removes a latent atomicity landmine and deletes the `Multi`-unpack VecDeque walk
- **Effort**: 2/5 — channel signature change, `scan` simplification, two call sites (worker.rs, session.rs)
- **Current**: Callers group messages with `WorkerMessage::Multi { msgs: Vec<WorkerMessage> }` (worker.rs:63). The worker reads up to 10 messages per `recv_many` and unpacks nested `Multi` during scan (worker.rs:186, 204). `send_many` itself wraps its argument in a single `Multi` (worker.rs:110–117).
- **Problem**: Atomicity of a batch is two coincidences away from breaking. If a caller ever sends 11+ loose messages, `recv_many` splits them across two commit phases silently. Atomicity is preserved today only because `session.rs` builds batches of ≤3 (session.rs:294–337) and wraps in `Multi`. `Multi` as an enum variant means the scanner carries a `VecDeque` just to flatten nesting. Two orthogonal mechanisms (channel batching + enum nesting) to express one concept ("this commit unit").
- **Alternative**: Change the channel to `UnboundedSender<Vec<WorkerMessage>>`. One send = one commit unit, period. `send_many` sends a `Vec` directly. `WorkerMessage::Multi` is deleted. `WORKER_MSG_BATCH` is deleted. `scan` walks a flat `Vec` instead of a `VecDeque`. The contract becomes load-bearing in the type: a batch *is* the `Vec`, not "whatever `recv_many` happened to pull".
- **Recommendation**: Do it. The current arrangement compiles but the semantics are fragile; the refactor is local and the API survives unchanged for `send` / `send_many`.

### [F2] Batch reduction rules are implicit across several pairs — DONE
- **Category**: Contract
- **Status**: Landed — `BatchIntent` now has `graph_state: Option<GraphOp>` and `loop_request: Option<LoopCommand>` with a module-level reduction table. Last-write-wins per slot, which collapses neatly: two `Update`s → last; `Clear` then `Update` → `Update`; `Update` then `Clear` → `Clear`; `Start` then `Stop` → `Stop`; `Stop` then `Start` → `Start`. Pinned by unit tests on `scan()` and one integration test for the Update-then-Clear end-to-end path.
- **Rule chosen**: **last-write-wins** for every slot that can be overwritten. Simpler and more consistent than per-slot "dominance" rules; matches the existing `Update(A) then Update(B) → B` behavior that callers already rely on.

### [F3] `is_event_loop_started` promises point-in-time truth it can't deliver
- **Category**: Contract / State
- **Impact**: 2/5 — works today because session polls at frame rate; API shape invites subtler bugs as callers multiply
- **Effort**: 2/5 — swap poll for push, one session field updates from callback
- **Current**: `event_loop_started: Arc<AtomicBool>` is stored at the top of the loop and again post-commit (worker.rs:232, 341). `session.rs:219` reads it each frame to sync autorun state.
- **Problem**: The value is coherent only at commit boundaries. If a `StartEventLoop` is queued but not yet scanned, the atomic reads `false` while the caller's mental model is "I just sent start, it should be true." Today that's fine — session reconciles on the next frame via the same poll. But there is no concept of "now"; the API pretends otherwise.
- **Alternative**: Deliver state transitions through the existing `execution_callback` (or a sibling callback). Worker fires `Status::EventLoopStarted` / `Status::EventLoopStopped` on the transition. Session mirrors the latest into its own field. The atomic disappears. This also matches how `ExecutionFinished` is already plumbed through `WorkerEvent` in session.rs:38.
- **Recommendation**: Do it if you touch this file for unrelated reasons. Not urgent standalone.

### [F4] `worker_loop` is conflating command dispatch with event-loop lifecycle
- **Category**: Responsibility
- **Impact**: 2/5 — readability, not correctness
- **Effort**: 2/5 — extract a struct, move three functions onto it
- **Current**: The loop owns `execution_graph`, the optional `(EventLoopHandle, Receiver<EventRef>)` tuple, and the `PauseGate` (worker.rs:223–229). `start_event_loop` and `reset_event_loop` are free functions passed the tuple by `&mut`.
- **Problem**: The "event loop subsystem" is three separate pieces (`EventLoopHandle`, the `Receiver`, the `PauseGate`) that must be manipulated in lockstep. `EventLoopHandle::stop` + drop-the-receiver is a two-step invariant with no type enforcing it. The in-loop tuple `Option<(EventLoopHandle, Receiver<EventRef>)>` is a minor smell.
- **Alternative**: `struct EventLoopDriver { handle: EventLoopHandle, events: Receiver<EventRef>, pause_gate: PauseGate }` with `start(triggers) -> Self`, `stop(self)`, `recv_batch(&mut self, buf)`. Worker_loop holds `Option<EventLoopDriver>`. The close-rx-with-handle invariant becomes "dropping the driver aborts tasks and drops the channel", enforced by `Drop`. `reset_event_loop` becomes `drop(driver.take())`.
- **Recommendation**: Do it when next modifying event-loop behavior; not worth a standalone PR.

### [F5] Events channel capacity = command batch size by coincidence
- **Category**: Data structures
- **Impact**: 1/5 — speculative
- **Effort**: 1/5 — rename + independent constants
- **Current**: Both `WORKER_MSG_BATCH` and `EVENT_LOOP_BACKPRESSURE` are `10` (worker.rs:22–27).
- **Problem**: These control unrelated things (per-poll command batch vs. per-lambda backpressure), happen to share a value, and are used interchangeably as the capacity of `ev_buf` and the event `channel`. Touching one invites breaking the other.
- **Alternative**: If F1 lands, `WORKER_MSG_BATCH` disappears. `EVENT_LOOP_BACKPRESSURE` stands alone and can be tuned independently.
- **Recommendation**: Drops out for free with F1.

## Rethink

Not needed. Two adjustments land 80% of the value:
1. **Channel carries `Vec<WorkerMessage>`** → kills `Multi`, kills `WORKER_MSG_BATCH`, makes "a batch is an atomic commit" type-enforced (F1 + F5).
2. **`BatchIntent.graph_state: Option<GraphOp>`** → one field for Clear/Update with last-write-wins semantics that match send order (F2).

After those, the loop's scan/commit shape is genuinely hard to improve on for this problem.

## Considered and rejected

- **Per-command optional ack (`oneshot::Sender` on every mutating variant)**. Would remove the `Ack` variant but bloat every other variant and every call site. `Ack` is three lines and composes fine with `send_many`; the duplication cost is lower than the uniformity cost. Keep.
- **Split `BatchIntent` into a single replay log (`Vec<ScannedOp>`) and re-execute at commit**. General and pretty, but `execution_graph.execute` is expensive enough that coalescing events into a HashSet and deduplicating Updates (both of which `BatchIntent` does today) is worth more than ordering purity. Keep.
- **Remove `PauseGate` and rely on task cancellation + fresh channels**. Tempting since stale-event handling is already structural. But `PauseGate` prevents new events from being *produced* during commit, which stops an unbounded backlog building in the bounded channel while `execute()` runs. Different job, keep.
