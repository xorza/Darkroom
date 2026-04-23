# Design review: scenarium/src/worker/  (2026-04-23)

## Current design

The worker owns an `ExecutionGraph` on a dedicated tokio task. Its single
public input is an unbounded channel of `Vec<WorkerMessage>` batches;
each batch is reduced to a `BatchIntent` (slots for graph op, loop
command, events, acks, argument requests, and an exit flag), then the
commit phase applies the intent in a fixed order: reset the running
event loop if anything touches it → apply graph op → run execute for
pending events → start a new loop if requested → serve argument queries
→ fire acks. Lambda-produced events arrive on a *separate* bounded
channel `Receiver<EventRef>` (backpressure = 10), selected with
`biased` so commands can't be starved. A `PauseGate` freezes all
lambdas while the commit phase runs. `event_loop_started` is exposed as
an `Arc<AtomicBool>` for outside polling.

The load-bearing decisions: (1) batches are atomic — one `send_many`
= one commit — which lets the session submit Update+ExecuteTerminals+
StartEventLoop in one frame and get exactly one teardown, one graph
rebuild, and one loop start. (2) The pause gate plus the bounded event
channel make the worker stop draining events during a commit, which
forces lambdas to block on `event_tx.send` → gives commands priority
without dropping events. (3) Teardown of the `(handle, Receiver)` pair
together guarantees in-flight events from the old loop can never
re-enter a new commit cycle.

State is a single `ExecutionGraph`, plus the optional
`(EventLoopHandle, Receiver<EventRef>)`. Everything else — intent,
batch buffer, event buffer — is local to the loop body.

## Overall take

The core approach is right. A single-threaded actor with a scan/commit
reducer is the correct shape for a stateful execution service with
conflicting overlapping commands. The module earned its `BatchIntent`
the hard way — I can see the reduction-rule table in the comments and
the "last-write-wins" discipline is consistent. The two-channel split
(commands vs lambda events) with biased select + pause gate is
genuinely well thought out.

What I'd push on: the **public API** (`WorkerMessage` enum) has drifted
to accommodate tests, and two of its variants (`Event`/`Events`, `Ack`)
aren't used by prod callers. That leak is worth fixing while things are
small. The commit phase also does some work twice that could collapse.

## Findings

### [F1] `WorkerMessage::Event` is redundant with `Events`; `Ack` has no current caller; both need intent-clear naming

- **Category**: Abstraction / Contract
- **Impact**: 2/5 — narrows the enum and signals intent; no behavior change
- **Effort**: 1/5 — rename + drop one variant + test edits
- **Current**: `WorkerMessage::Event` and `WorkerMessage::Events` have 15 refs between them, all in `scenarium/src/worker/tests.rs`. `WorkerMessage::Ack` has 8 refs, also all in tests. The only non-test sender is `prism/src/session.rs:294-342`, which pushes {`StartEventLoop`, `StopEventLoop`, `ExecuteTerminals`, `Update`, `RequestArgumentValues`, `Clear`}. In prod, events reach the worker exclusively through the internal `Receiver<EventRef>` spawned by `start_event_loop` (`scenarium/src/worker/mod.rs:416`).
- **Problem**: `Event`/`Events` are *public* injection entry points that duplicate each other — the singular form is a special case of the plural. Keeping both invites "which one do I use?" confusion at the call site. Their current name also reads as "this is how events flow," which is misleading — events normally flow on the internal channel; the `WorkerMessage` path is a side-door for external injection (scripting, replay, tests). `Ack` has no prod caller today but is a genuinely useful external sync primitive (scripting sequencing, graceful shutdown barriers, idle indicators) that would be painful to reintroduce cleanly later.
- **Decision**:
  1. **Drop `WorkerMessage::Event`** — redundant with the plural form. Tests using it become `InjectEvents { events: vec![e] }`.
  2. **Rename `Events` → `InjectEvents`** — makes the "external side-door injection" intent explicit at the call site so it doesn't read as the normal event path.
  3. **Keep `Ack`** — reserved for external sync. No current prod caller, but costs one enum variant and one trivial scan arm; the anticipated driver is the script transport (`prism/src/script/`). If no external caller materializes, strip it in a future pass.
- **Recommendation**: Do it. Net change: one variant removed, one renamed, one kept with a clearer contract. Reduction table in `mod.rs:187-195` loses the `Event|Events` row in favor of a single `InjectEvents` row.

### [F2] `send` allocates a `Vec` per single message; `send`/`send_many` duplication encodes a guarantee no caller needs

- **Category**: Abstraction / Data structures
- **Impact**: 3/5 — kills an allocation per single-message send and removes a subtle API pitfall (is `send([a, b])` the same as `send(a); send(b)`? today: no)
- **Effort**: 3/5 — refactor within the module + adjust callers. Tests will need small changes.
- **Current**: Wire format is `Vec<WorkerMessage>` (mod.rs:71). `Worker::send` wraps every message in `vec![msg]` (mod.rs:102). `send_many` is the "atomic commit" primitive. The guarantee it documents is "one send = one commit unit," i.e. two `send_many` calls won't interleave their reduction.
- **Problem**: Only one thread (session) sends to the worker in prod. For that caller, "don't interleave" is free — they don't hold a partially-built batch across an await or hand it across threads. Paying `vec![msg]` per single-message send and exposing two methods (`send` + `send_many`) with different semantic weights is overhead that buys nothing. Tests are the only callers that care about atomicity, and even there the need is only "don't let the worker peek at my messages one at a time" — which can be achieved differently.
- **Alternative(s)**:
  1. Make the channel carry `WorkerMessage` directly. In the loop, on wake, drain: `msgs.push(first); while let Ok(m) = rx.try_recv() { msgs.push(m) }`. Reduce that buffer. Atomicity becomes "everything that was in the channel at wake time collapses together" — strictly *more* batching than today, at no cost. The reduction math is the same: the scan is commutative/last-write-wins per slot, except for Ack/arg_requests which are order-preserving lists, and `try_recv` preserves arrival order.
  2. Keep `Vec<WorkerMessage>` but delete `Worker::send` and force all callers through `send_many`. Cheap but doesn't fix the alloc.
  3. Expose a builder: `worker.batch().update(...).execute_terminals().commit()`. Type-enforces batch construction; moves the `Vec` inside the builder. Pairs naturally with F1 (builder methods are typed, no enum exposed to callers).
- **Recommendation**: Do (1). It's the simplest, it strictly widens batching, and it removes both the alloc and the duplicate API. (3) is also defensible if you liked the "atomic batch" vocabulary — but I don't think the caller benefits from naming the boundary.

### [F3] `event_loop_started: Arc<AtomicBool>` is observable cross-thread state duplicating internal state

- **Category**: State
- **Impact**: 2/5 — removes a sync point; minor
- **Effort**: 2/5 — one call site in prism
- **Current**: `event_loop_started` is written at `mod.rs:269` and `mod.rs:380` and read by `prism/src/session.rs:221` to render UI state. It's `Ordering::Relaxed` on both sides and is effectively a single-bit cache of `event_loop.is_some()`.
- **Problem**: Every state exposed as a shared atomic is a place callers can get a stale or torn view. Today the worker publishes execution results via an `ExecutionCallback` the caller already owns; "event loop started/stopped" is the same kind of notification and could ride the same channel. Having two publication paths (callback for exec results, atomic for loop state) is minor duplication.
- **Alternative(s)**:
  1. Widen the callback's payload: `enum WorkerEvent { Executed(Result<Stats>), LoopStarted, LoopStopped }`. Session mirrors the state locally.
  2. Keep the atomic but assert it's only set in one place per iteration. The current loop writes it twice per iteration (start+end) defensively; pick one.
- **Recommendation**: Depends on whether other "worker state" snapshots grow. If this stays the only one, leave it. If you add even one more (e.g., "graph empty"), switch to (1) — two atomics is where this pattern breaks down.

### [F4] Start-loop path double-executes the graph and double-fires the callback

- **Category**: Control flow
- **Impact**: 3/5 — user-visible: callback sees two results in one tick; subtle duplicate work
- **Effort**: 2/5 — localized to commit phase
- **Current**: When one batch contains both events and `StartEventLoop` (or `ExecuteTerminals` + `StartEventLoop`), the commit does:
  - `mod.rs:337-345` — `execution_graph.execute(execute_terminals, /*in_loop=*/false, events)`, then `execution_callback(result)`.
  - `mod.rs:354` — `execution_graph.execute(false, /*in_loop=*/true, [])`, then `execution_callback(result)`.
  The same post-commit graph is executed twice back-to-back, differing only in the `in_loop` flag, and the caller's callback fires twice.
- **Problem**: Two execution results per tick in the start-loop-together-with-exec case. The second execution exists to produce `ExecutionStats` so `active_event_triggers(stats)` can pick the triggers to spawn. But we already did an execution that produced stats; we throw them away.
- **Alternative(s)**:
  1. Fold the "pre-spawn execute" into the first execution. When `should_start_event_loop && !execution_graph.is_empty()`, ensure the first `execute()` call uses `in_loop=true` and we reuse its stats for `active_event_triggers`. If no events/terminals are pending, *still* do the single execute for the start path.
  2. Have `start_event_loop` compute triggers from a cheaper source (not requiring a full exec). Unclear if feasible without deeper inspection; (1) is the low-risk move.
- **Recommendation**: Do (1). It collapses the commit phase from "maybe-execute, maybe-execute" to "at most one execute" and stops firing the callback twice.

### [F5] `EmptyGraph` is reported as an `Err` for every event/terminal request while the graph is empty

- **Category**: Contract
- **Impact**: 2/5 — ergonomics; callers filter for this
- **Effort**: 2/5
- **Current**: `mod.rs:333-345` and `mod.rs:351-368` both fire `execution_callback(Err(Error::EmptyGraph))` whenever events/terminals/start are requested on an empty graph. A single batch with three events + start-loop on an empty graph produces two error callbacks (one for events branch, one for start-loop branch).
- **Problem**: "Empty graph" is a state, not a failure. The caller's `ExecutionCallback` gets spammed with a sentinel error that is really "no-op, try later." Mixing this into `Result<ExecutionStats>` forces every callback to pattern-match for a condition the worker already knows about.
- **Alternative(s)**:
  1. Make empty-graph a silent no-op: skip the callback entirely. Callers that care can query graph emptiness separately.
  2. Separate the condition: `execution_callback` handles actual exec results; surface "empty graph" via a logged event or a dedicated `on_empty` callback.
- **Recommendation**: Do (1), unless there's a caller relying on the `EmptyGraph` error. Quick grep suggests session treats it as just another result to display — which means the user sees "Error: EmptyGraph" for what is actually a normal state. (Worth confirming with a callsite check before ripping it out.)

### [F6] Pause gate closes across *every* commit, including ones that don't touch the event loop

- **Category**: Control flow
- **Impact**: 2/5 — correctness is fine; a latency cost on lambdas when Ack/arg-query batches land
- **Effort**: 2/5
- **Current**: `mod.rs:299` — `event_loop_pause_gate.close()` runs unconditionally at the top of every commit iteration, then drops at `mod.rs:386`. This pauses *all* lambdas for the duration of the commit phase even when the batch is just `Ack` or `RequestArgumentValues` — neither of which interacts with the running loop.
- **Problem**: Over-broad gating. On a batch of pure queries, there's no reason lambdas can't keep producing events; they'd just pile up in the bounded channel up to backpressure. The gate is only load-bearing for batches that (a) mutate the graph or (b) toggle the loop — both already go through `reset_event_loop`, which aborts every task.
- **Alternative(s)**:
  1. Only close the pause gate when `needs_reset || intent.execute_terminals || !intent.events.is_empty()`. Query-only batches don't touch it.
  2. Remove the gate entirely. Teardown already uses `abort().await`, which is a stronger barrier than the gate. The only reason to keep the gate is if lambda state-mutation races with `get_argument_values_with_previews` — which needs a focused look.
- **Recommendation**: Start with (1) — narrow the scope. (2) requires more thought because the gate may be intentional mutual exclusion with `SharedAnyState` readers; I didn't trace that deeply.

## Rethink

If I were writing this today, the shape is roughly:

```rust
pub struct Worker { /* opaque */ }

impl Worker {
    pub fn new(on_event: impl Fn(WorkerEvent) + Send + 'static) -> Self;

    // Fire-and-forget commands. Internally enqueued on a single mpsc.
    pub fn update(&self, graph: Graph, func_lib: Arc<FuncLib>);
    pub fn clear(&self);
    pub fn execute_once(&self);        // was ExecuteTerminals
    pub fn start_loop(&self);
    pub fn stop_loop(&self);

    // Queries return futures.
    pub fn argument_values(&self, id: NodeId)
        -> impl Future<Output = Option<ArgumentValues>>;
}

pub enum WorkerEvent {
    Executed(Result<ExecutionStats>),
    LoopStarted,
    LoopStopped,
    // (no EmptyGraph)
}

#[cfg(test)]
impl Worker {
    pub fn flush(&self) -> impl Future<Output = ()>;
    pub fn inject_event(&self, e: EventRef);
}
```

Wire side: one `mpsc::UnboundedSender<WorkerMessageInternal>`, drained
fully on each wake via `recv().await` + `try_recv()` loop, folded into
the existing `BatchIntent` shape. Everything the reducer does today
stays — the `BatchIntent` struct with its named slots is the right
pattern for this problem and shouldn't change.

Net: enum shrinks by ~3 variants, allocation-per-send disappears, two
observable state paths collapse to one, callback no longer double-fires
on loop start, and the pause gate narrows to its real scope. The core
actor-with-reducer shape is unchanged.

## Considered and rejected

- **Split the worker into two actors (graph-state actor + loop-runner actor).** The current single-actor design has exactly the right grain: the graph and the loop are intertwined (loop consumes graph, graph mutations require loop teardown). Splitting would force a shared-state dance between them that's strictly worse than the current "one owner, one commit phase."
- **Replace `BatchIntent` with a linear event log the commit phase walks.** Losing last-write-wins reduction means replaying Stop→Start→Stop→Start on every frame where the user double-clicks. The reducer model is the correct pattern.
- **Make `ExecutionGraph::execute` return `active_event_triggers` directly.** This would fix F4 but leaks loop-specific output into a general method. F4's fix (single execute call with `in_loop=true`) is strictly better.
