use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use common::PauseGate;
use tokio::sync::{Notify, mpsc, oneshot};
use tokio::time::{Duration, timeout};

use crate::StaticValue;
use crate::elements::system_library::system_library;
use crate::elements::worker_events_library::worker_events_library;
use crate::execution::Result as ExecResult;
use crate::execution::compile::{CompiledGraph, Compiler};
use crate::execution::identity::NodeAddress;
use crate::execution::report::RunPhase;
use crate::execution::stats::ExecutionStats;
use crate::graph::{Binding, Graph, InputPort, Node, NodeId, NodeSearch};
use crate::library::Library;
use crate::node::event::EventLambda;
use crate::runtime::shared_any_state::SharedAnyState;

use crate::execution::event::{EventRef, EventTrigger};
use crate::worker::Worker;
use crate::worker::batch::{BatchIntent, GraphOp, LoopCommand, scan};
use crate::worker::event_loop::ActiveEventLoop;
use crate::worker::protocol::{WorkerMessage, WorkerReport};

/// Print messages a run logged, in order — `print` now logs via
/// `ContextManager::info`, surfaced in `ExecutionStats.logs`.
fn messages(stats: &ExecutionStats) -> Vec<String> {
    stats.logs.iter().map(|e| e.message.clone()).collect()
}

/// End-to-end fixture for Worker tests that run a graph with a
/// `frame event` source and a sink `print`. Builds the func lib,
/// the standard 3-node graph, and a worker whose callback forwards
/// results into an mpsc; exposes helpers for the two messages used
/// most often (`Update` with the fixture graph; a frame-event
/// `EventRef`).
struct FrameHarness {
    worker: Worker,
    library: Arc<Library>,
    graph: Graph,
    frame_event_node_id: NodeId,
    compute_rx: mpsc::Receiver<ExecResult<ExecutionStats>>,
}

impl FrameHarness {
    async fn new() -> Self {
        Self::with_callback_capacity(8).await
    }

    async fn with_callback_capacity(cap: usize) -> Self {
        let mut library = system_library();
        library.merge(worker_events_library());

        let graph = log_frame_no_graph(&library);
        let frame_event_node_id = graph
            .find_by_name("Frame Event", NodeSearch::TopLevel)
            .unwrap()
            .id;
        let library = Arc::new(library);

        let (worker, compute_rx) = finished_worker(cap);

        Self {
            worker,
            library,
            graph,
            frame_event_node_id,
            compute_rx,
        }
    }

    fn update_msg(&self) -> WorkerMessage {
        WorkerMessage::Update {
            compiled: Compiler::default()
                .compile(&self.graph, &self.library)
                .unwrap(),
        }
    }

    fn frame_event(&self) -> EventRef {
        EventRef {
            node_id: self.frame_event_node_id,
            event_idx: 0,
        }
    }

    fn inject_frame_event(&self) -> WorkerMessage {
        WorkerMessage::InjectEvents {
            events: vec![self.frame_event()],
        }
    }
}

fn log_frame_no_graph(library: &Library) -> Graph {
    let mut graph = Graph::default();

    let frame_event_node_id: NodeId = "e69c3f32-ac66-4447-a3f6-9e8528c5d830".into();
    let to_string_node_id: NodeId = "eb6590aa-229d-4874-abba-37c56f5b97fa".into();
    let print_node_id: NodeId = "8be72298-dece-4a5f-8a1d-d2dee1e791d3".into();

    let frame_event_func = library.by_name("Frame Event").unwrap();
    let to_string_func = library.by_name("To String").unwrap();
    let print_func = library.by_name("Print").unwrap();

    let frame_event_node: Node = frame_event_func.into();
    graph.insert(frame_event_node_id, frame_event_node);

    let to_string_node: Node = to_string_func.into();
    graph.insert(to_string_node_id, to_string_node);

    let print_node: Node = print_func.into();
    graph.insert(print_node_id, print_node);

    graph.set_input_binding(InputPort::new(frame_event_node_id, 0), 1.into());
    graph.subscribe(frame_event_node_id, 0, print_node_id);
    graph.set_input_binding(
        InputPort::new(to_string_node_id, 0),
        Binding::bind(frame_event_node_id, 1),
    );
    graph.set_input_binding(
        InputPort::new(print_node_id, 0),
        Binding::bind(to_string_node_id, 0),
    );

    graph
}

/// A single `Print` sink node bound to a literal string — the minimal graph
/// several tests use when they only care about the run completing and its
/// log line, not the graph's shape.
fn print_literal_graph(library: &Library, message: &str) -> (Graph, NodeId) {
    let mut graph = Graph::default();
    let print_node: Node = library.by_name("Print").unwrap().into();
    let print_node_id = graph.add(print_node);
    graph.set_input_binding(
        InputPort::new(print_node_id, 0),
        StaticValue::String(message.to_string()).into(),
    );
    (graph, print_node_id)
}

/// Start an event loop with a single lambda as its only trigger, on a fresh
/// `NodeId` — the shape most `start_event_loop` tests want when they only
/// care about one lambda's behavior.
async fn start_single_event_loop(
    lambda: EventLambda,
    pause_gate: PauseGate,
) -> (ActiveEventLoop, NodeId) {
    let node_id = NodeId::unique();
    let active = ActiveEventLoop::start(
        vec![EventTrigger {
            event: EventRef {
                node_id,
                event_idx: 0,
            },
            lambda,
            state: SharedAnyState::default(),
        }],
        pause_gate,
    )
    .await;
    (active, node_id)
}

/// A worker whose callback forwards only `Finished` reports into a fresh
/// mpsc of capacity `cap` — the shape most tests want when they don't care
/// about live progress. Works with or without a graph loaded.
fn finished_worker(cap: usize) -> (Worker, mpsc::Receiver<ExecResult<ExecutionStats>>) {
    let (tx, rx) = mpsc::channel(cap);
    let worker = Worker::new(move |report| {
        if let WorkerReport::Finished(result) = report {
            tx.try_send(result).ok();
        }
    });
    (worker, rx)
}

/// Send `msgs` plus a trailing `Sync`, and block until the batch has fully
/// committed. Panics on timeout or a dropped reply — every call site here
/// expects the worker to still be alive and processing.
async fn sync_after(worker: &Worker, msgs: impl IntoIterator<Item = WorkerMessage>) {
    let (reply, rx) = oneshot::channel();
    worker
        .send_many(msgs.into_iter().chain([WorkerMessage::Sync { reply }]))
        .unwrap();
    timeout(Duration::from_millis(500), rx)
        .await
        .expect("sync timed out")
        .expect("sync sender dropped");
}

/// A unique temp dir removed on drop, so tests don't collide or leak.
/// `prefix` disambiguates directories from different tests sharing the
/// process-wide temp dir.
struct TempDir(std::path::PathBuf);

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn temp_dir(prefix: &str) -> TempDir {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let p = std::env::temp_dir().join(format!(
        "scenarium-worker-{prefix}-{}-{n}",
        std::process::id()
    ));
    std::fs::create_dir_all(&p).unwrap();
    TempDir(p)
}

#[tokio::test]
async fn test_worker() -> anyhow::Result<()> {
    let mut h = FrameHarness::new().await;

    h.worker
        .send_many([h.update_msg(), h.inject_frame_event()])
        .unwrap();

    for expected in ["1", "2", "3"] {
        if expected != "1" {
            h.worker.send(h.inject_frame_event()).unwrap();
        }
        let executed = h
            .compute_rx
            .recv()
            .await
            .expect("Missing compute completion")
            .expect("Unsuccessful compute");
        assert_eq!(executed.executed_nodes.len(), 3);
        assert_eq!(messages(&executed), [expected]);
    }

    Ok(())
}

#[tokio::test]
async fn start_event_loop_forwards_events() {
    let event_lambda = EventLambda::new(|_state| Box::pin(async move {}));
    let (mut active, node_id) = start_single_event_loop(event_lambda, PauseGate::default()).await;

    let event = active
        .events
        .recv()
        .await
        .expect("Expected event loop event");
    assert_eq!(
        event,
        EventRef {
            node_id,
            event_idx: 0
        }
    );

    active.stop().await;
}

#[tokio::test]
async fn start_event_loop_waits_for_callback() {
    let notify = Arc::new(Notify::new());
    let notify_for_event = Arc::clone(&notify);
    let event_lambda = EventLambda::new(move |_state| {
        let notify = Arc::clone(&notify_for_event);
        Box::pin(async move {
            notify.notified().await;
        })
    });

    let notify_for_callback = Arc::clone(&notify);

    let (mut active, node_id) = start_single_event_loop(event_lambda, PauseGate::default()).await;

    notify_for_callback.notify_waiters();

    let event = timeout(Duration::from_millis(200), active.events.recv())
        .await
        .expect("Expected event")
        .expect("Event channel closed");
    assert_eq!(
        event,
        EventRef {
            node_id,
            event_idx: 0
        }
    );

    active.stop().await;
}

#[tokio::test]
async fn pause_gate_blocks_event_loop_iterations() {
    use std::sync::atomic::{AtomicUsize, Ordering};

    let invoke_count = Arc::new(AtomicUsize::new(0));
    let invoke_count_clone = Arc::clone(&invoke_count);

    let event_lambda = EventLambda::new(move |_state| {
        let invoke_count = Arc::clone(&invoke_count_clone);
        Box::pin(async move {
            invoke_count.fetch_add(1, Ordering::SeqCst);
        })
    });

    let pause_gate = PauseGate::default();

    let (mut active, _node_id) = start_single_event_loop(event_lambda, pause_gate.clone()).await;

    // Wait for first event to arrive
    let _ = timeout(Duration::from_millis(100), active.events.recv())
        .await
        .expect("Expected first event");

    // Close the gate - event loop should pause
    let _guard = pause_gate.close();

    // Record count after closing gate
    tokio::time::sleep(Duration::from_millis(20)).await;
    let count_at_close = invoke_count.load(Ordering::SeqCst);

    // Wait and verify no new invocations while gate is closed
    tokio::time::sleep(Duration::from_millis(100)).await;
    let count_while_closed = invoke_count.load(Ordering::SeqCst);

    // At most one more invocation might have slipped through
    assert!(
        count_while_closed <= count_at_close + 1,
        "Event loop should pause when gate is closed. Count at close: {}, count while closed: {}",
        count_at_close,
        count_while_closed
    );

    // Drop guard to reopen gate
    drop(_guard);

    // Wait for more events to flow
    tokio::time::sleep(Duration::from_millis(100)).await;
    let count_after_reopen = invoke_count.load(Ordering::SeqCst);

    assert!(
        count_after_reopen > count_while_closed,
        "Event loop should resume after gate reopens. Count while closed: {}, count after reopen: {}",
        count_while_closed,
        count_after_reopen
    );

    active.stop().await;
}

#[tokio::test]
async fn lambda_panic_is_captured_not_unwound() {
    // A panicking event lambda must be isolated: `stop()` captures the panic
    // (attributed to its node) and returns it, rather than unwinding into the
    // worker loop — which would kill the worker.
    let event_lambda = EventLambda::new(|_state| Box::pin(async { panic!("boom in lambda") }));
    let (mut active, node_id) = start_single_event_loop(event_lambda, PauseGate::default()).await;

    // The lambda panics on its first invoke and never sends; its sole sender
    // drops, closing the channel. Awaiting that close ensures the panic has
    // landed before we stop.
    assert!(
        active.events.recv().await.is_none(),
        "panicking lambda should close the event channel without sending"
    );

    let panics = active.stop().await;
    assert_eq!(panics.len(), 1, "the lambda panic should be captured");
    assert_eq!(panics[0].node_id, node_id, "panic attributed to its node");
    assert!(
        panics[0].message.contains("boom in lambda"),
        "panic message preserved: {}",
        panics[0].message
    );
}

#[tokio::test]
async fn clear_resets_execution_graph() {
    let mut h = FrameHarness::new().await;

    h.worker
        .send_many([h.update_msg(), h.inject_frame_event()])
        .unwrap();
    let stats = h.compute_rx.recv().await.unwrap().unwrap();
    assert_eq!(messages(&stats), ["1"]);

    h.worker.send(WorkerMessage::Clear).unwrap();
    h.worker.send(h.inject_frame_event()).unwrap();

    tokio::time::sleep(Duration::from_millis(50)).await;
    // After Clear the frame event has no subscribers, so nothing runs /
    // logs — drain any callbacks and assert no print output.
    let mut printed = Vec::new();
    while let Ok(result) = h.compute_rx.try_recv() {
        if let Ok(stats) = result {
            printed.extend(messages(&stats));
        }
    }
    assert!(printed.is_empty());
}

#[tokio::test]
async fn events_are_deduplicated() {
    let mut h = FrameHarness::new().await;

    h.worker.send(h.update_msg()).unwrap();

    let event = h.frame_event();
    h.worker
        .send(WorkerMessage::InjectEvents {
            events: vec![event, event, event],
        })
        .unwrap();

    let stats = h.compute_rx.recv().await.unwrap().unwrap();
    assert_eq!(messages(&stats), ["1"]);
}

#[tokio::test]
async fn execute_sinks_triggers_sink_nodes() {
    let library = system_library();
    // Simple single-sink graph — doesn't use FrameHarness' frame-event setup.
    let (graph, _print_id) = print_literal_graph(&library, "hello");

    let (worker, mut compute_finish_rx) = finished_worker(8);
    worker
        .send_many([
            WorkerMessage::Update {
                compiled: Compiler::default().compile(&graph, &library).unwrap(),
            },
            WorkerMessage::ExecuteSinks,
        ])
        .unwrap();

    let executed = compute_finish_rx
        .recv()
        .await
        .expect("Missing compute completion")
        .expect("Unsuccessful compute");

    assert_eq!(executed.executed_nodes.len(), 1);
    assert_eq!(messages(&executed), ["hello"]);
}

#[tokio::test(flavor = "multi_thread")]
async fn worker_streams_node_progress_before_finished() {
    let library = system_library();
    let (graph, print_node_id) = print_literal_graph(&library, "hi");

    // Capture the full report stream (progress + final), unlike the fixture.
    let (tx, mut rx) = mpsc::channel::<WorkerReport>(16);
    let worker = Worker::new(move |report| {
        tx.try_send(report).ok();
    });
    worker
        .send_many([
            WorkerMessage::Update {
                compiled: Compiler::default().compile(&graph, &library).unwrap(),
            },
            WorkerMessage::ExecuteSinks,
        ])
        .unwrap();

    // The single node's Started + Finished progress both arrive (mapped to its
    // authoring id) ahead of the sink `Finished` report.
    let mut started = 0;
    let mut node_finished = 0;
    loop {
        let report = timeout(Duration::from_secs(5), rx.recv())
            .await
            .expect("worker timed out")
            .expect("worker channel closed");
        match report {
            WorkerReport::Progress(p) => {
                assert_eq!(p.node_id, print_node_id, "progress maps to the node");
                match p.phase {
                    RunPhase::Started { .. } => started += 1,
                    RunPhase::Finished { .. } => node_finished += 1,
                }
            }
            WorkerReport::PinnedOutputs(_) => {}
            WorkerReport::Finished(result) => {
                assert_eq!(result.expect("compute ok").executed_nodes.len(), 1);
                break;
            }
        }
    }
    assert_eq!(started, 1, "one Started before the final report");
    assert_eq!(
        node_finished, 1,
        "one Finished progress before the final report"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn stale_cancel_is_cleared_at_run_start() {
    let library = system_library();
    let (graph, _print_node_id) = print_literal_graph(&library, "hi");

    let (worker, mut rx) = finished_worker(8);

    // Cancel requested with nothing running: it must not bleed into the run
    // kicked next (the worker clears the flag at each run's start).
    worker.request_cancel();
    worker
        .send_many([
            WorkerMessage::Update {
                compiled: Compiler::default().compile(&graph, &library).unwrap(),
            },
            WorkerMessage::ExecuteSinks,
        ])
        .unwrap();

    let stats = timeout(Duration::from_secs(5), rx.recv())
        .await
        .expect("worker timed out")
        .expect("worker channel closed")
        .expect("compute ok");
    assert!(!stats.cancelled, "a stale cancel was cleared at run start");
    assert_eq!(stats.executed_nodes.len(), 1, "the run completed in full");
}

#[tokio::test]
async fn start_stop_event_loop() {
    let mut h = FrameHarness::with_callback_capacity(32).await;

    sync_after(&h.worker, [h.update_msg(), WorkerMessage::StartEventLoop]).await;

    let initial = h.compute_rx.recv().await.unwrap().unwrap();
    assert!(messages(&initial).is_empty());
    let event = timeout(Duration::from_millis(500), h.compute_rx.recv())
        .await
        .expect("event loop did not produce a callback")
        .expect("callback channel closed")
        .expect("event loop execution failed");
    assert_eq!(event.executed_nodes.len(), 3);
    assert_eq!(event.logs.len(), 1);

    sync_after(&h.worker, [WorkerMessage::StopEventLoop]).await;
    while h.compute_rx.try_recv().is_ok() {}
    assert_no_callback_within(&mut h.compute_rx, Duration::from_millis(100)).await;
}

/// `ExecuteNodes` end-to-end: an `Update` + `ExecuteNodes` batch runs only the seeded
/// node's cone (3 of the fixture's 5 nodes; the sink `Print` panics if reached).
#[tokio::test]
async fn execute_nodes_runs_only_the_seeded_cone() {
    use crate::graph::CacheMode;
    use crate::testing::{TestFuncHooks, test_func_lib, test_graph};

    let library = test_func_lib(TestFuncHooks {
        get_a: Arc::new(|| Ok(1)),
        get_b: Arc::new(|| 11),
        ..Default::default()
    });
    let mut graph = test_graph();
    for node in graph.nodes.values_mut() {
        node.cache = CacheMode::None;
    }
    let sum_id = graph.find_by_name("sum", NodeSearch::TopLevel).unwrap().id;

    let (worker, mut rx) = finished_worker(8);
    worker
        .send_many([
            WorkerMessage::Update {
                compiled: Compiler::default().compile(&graph, &library).unwrap(),
            },
            WorkerMessage::ExecuteNodes {
                nodes: vec![NodeAddress::root(sum_id)],
            },
        ])
        .unwrap();

    let stats = timeout(Duration::from_secs(5), rx.recv())
        .await
        .expect("worker timed out")
        .expect("worker channel closed")
        .expect("run ok");
    assert_eq!(stats.executed_nodes.len(), 3, "only sum's cone ran");
}

#[tokio::test]
async fn sync_fires_after_execution() {
    let mut h = FrameHarness::new().await;

    sync_after(&h.worker, [h.update_msg(), h.inject_frame_event()]).await;
    let _ = h.compute_rx.recv().await;
}

#[tokio::test]
async fn update_restarts_event_loop_if_running() {
    let mut h = FrameHarness::with_callback_capacity(32).await;

    sync_after(&h.worker, [h.update_msg(), WorkerMessage::StartEventLoop]).await;
    while h.compute_rx.try_recv().is_ok() {}
    timeout(Duration::from_millis(500), h.compute_rx.recv())
        .await
        .expect("event loop did not produce a callback before update")
        .expect("callback channel closed")
        .expect("event loop execution failed");

    sync_after(&h.worker, [h.update_msg()]).await;
    while h.compute_rx.try_recv().is_ok() {}
    let event = timeout(Duration::from_millis(500), h.compute_rx.recv())
        .await
        .expect("restarted event loop did not produce a callback")
        .expect("callback channel closed")
        .expect("event loop execution failed");
    assert_eq!(event.executed_nodes.len(), 3);
}

// Stale-event filtering is now structural: each start_event_loop
// call returns a fresh Receiver; stop_event_loop drops the old
// pair so any undelivered events die with the channel. This test
// verifies the structural guarantee by confirming the old
// Receiver is closed after its sibling handle is stopped.
#[tokio::test]
async fn stopped_event_loop_channel_is_closed() {
    let event_lambda = EventLambda::new(|_state| Box::pin(async move {}));
    let (mut active, _node_id) = start_single_event_loop(event_lambda, PauseGate::default()).await;

    active.stop().await;

    // After stop, all lambda tasks (the sole senders) are aborted →
    // the Receiver must observe channel closure. Drain under a
    // bounded per-recv timeout so a regression that stops closing
    // the channel fails fast instead of wedging the test.
    loop {
        let item = timeout(Duration::from_millis(500), active.events.recv())
            .await
            .expect("recv must complete — channel must eventually close after handle.stop()");
        if item.is_none() {
            break;
        }
    }
}

#[tokio::test]
async fn send_many_empty_is_noop() {
    // Empty batch must not panic, hang, or desynchronize the worker.
    let worker = Worker::new(|_| {});

    worker
        .send_many(std::iter::empty::<WorkerMessage>())
        .unwrap();

    // Subsequent Sync still fires → worker is alive.
    sync_after(&worker, std::iter::empty()).await;
}

#[tokio::test]
async fn stop_event_loop_when_not_running_is_noop() {
    let worker = Worker::new(|_| {});

    sync_after(&worker, [WorkerMessage::StopEventLoop]).await;
}

#[tokio::test]
async fn multiple_syncs_in_batch_all_run() {
    let worker = Worker::new(|_| {});

    let (reply_a, rx_a) = oneshot::channel();
    let (reply_b, rx_b) = oneshot::channel();

    worker
        .send_many([
            WorkerMessage::Sync { reply: reply_a },
            WorkerMessage::Sync { reply: reply_b },
        ])
        .unwrap();

    timeout(Duration::from_millis(500), rx_a)
        .await
        .expect("First Sync should fire")
        .expect("First sender dropped");
    timeout(Duration::from_millis(500), rx_b)
        .await
        .expect("Second Sync should fire")
        .expect("Second sender dropped");
}

#[tokio::test]
async fn clear_then_update_in_same_batch_applies_update() {
    // Scan-then-commit ordering: Clear zeroes execution_graph, Update
    // queues a replacement, commit phase applies Update and flips
    // execution_graph_clear back to false. The event must execute.
    let mut h = FrameHarness::new().await;

    h.worker
        .send_many([WorkerMessage::Clear, h.update_msg(), h.inject_frame_event()])
        .unwrap();

    let executed = h
        .compute_rx
        .recv()
        .await
        .expect("Missing compute completion")
        .expect("Unsuccessful compute");

    assert_eq!(executed.executed_nodes.len(), 3);
    assert_eq!(messages(&executed), ["1"]);
}

// F5: running execution on an empty graph is a normal state, not a
// failure — the worker must skip execute silently. No callback fires
// until a graph is loaded.

async fn assert_no_callback_within(
    rx: &mut mpsc::Receiver<ExecResult<ExecutionStats>>,
    d: Duration,
) {
    assert!(
        timeout(d, rx.recv()).await.is_err(),
        "unexpected worker callback"
    );
}

#[tokio::test]
async fn execute_sinks_on_empty_graph_is_silent_noop() {
    let (worker, mut rx) = finished_worker(8);
    worker.send(WorkerMessage::ExecuteSinks).unwrap();
    assert_no_callback_within(&mut rx, Duration::from_millis(100)).await;
}

#[tokio::test]
async fn event_on_empty_graph_is_silent_noop() {
    let (worker, mut rx) = finished_worker(8);
    worker
        .send(WorkerMessage::InjectEvents {
            events: vec![EventRef {
                node_id: NodeId::unique(),
                event_idx: 0,
            }],
        })
        .unwrap();
    assert_no_callback_within(&mut rx, Duration::from_millis(100)).await;
}

#[tokio::test]
async fn start_event_loop_on_empty_graph_is_silent_noop() {
    let (worker, mut rx) = finished_worker(8);
    sync_after(&worker, [WorkerMessage::StartEventLoop]).await;
    assert_no_callback_within(&mut rx, Duration::from_millis(100)).await;
}

// F4 regression: when a batch triggers both an execution
// (ExecuteSinks or InjectEvents) and StartEventLoop, the commit
// phase must run execute() once and fire the callback once — not twice.

#[tokio::test]
async fn execute_sinks_with_start_event_loop_fires_callback_once() {
    // Sink-only graph: active_event_triggers() returns empty, so
    // the loop never actually spawns. This removes lambda-driven
    // callbacks as a confounding factor while still exercising the
    // should_start_event_loop branch.
    let library = system_library();
    let (graph, _print_node_id) = print_literal_graph(&library, "hi");

    let (worker, mut compute_finish_rx) = finished_worker(8);

    worker
        .send_many([
            WorkerMessage::Update {
                compiled: Compiler::default().compile(&graph, &library).unwrap(),
            },
            WorkerMessage::ExecuteSinks,
            WorkerMessage::StartEventLoop,
        ])
        .unwrap();

    let first = timeout(Duration::from_millis(500), compute_finish_rx.recv())
        .await
        .expect("batch must fire callback")
        .expect("callback channel closed");
    assert!(first.is_ok(), "execute must succeed: {first:?}");

    assert_no_callback_within(&mut compute_finish_rx, Duration::from_millis(100)).await;
    assert_eq!(messages(first.as_ref().unwrap()), ["hi"]);
}

// F2: when two separate send_many calls queue messages before the
// worker wakes, the worker's drain-on-wake pulls both into a single
// BatchIntent — reducing per-slot. Under current_thread scheduling,
// the test task's synchronous sends all land in the channel before
// the worker task is polled, so this tests the drain path
// deterministically.
#[tokio::test(flavor = "current_thread")]
async fn drain_on_wake_folds_queued_batches_into_one_commit() {
    let library = system_library();

    // Sink-only graph — one execute produces one line of output.
    let (graph, _print_node_id) = print_literal_graph(&library, "once");

    let (worker, mut compute_finish_rx) = finished_worker(8);

    // Three separate send_many calls, all synchronous — they all
    // land in the channel before the worker task is polled.
    worker
        .send_many([WorkerMessage::Update {
            compiled: Compiler::default().compile(&graph, &library).unwrap(),
        }])
        .unwrap();
    worker.send_many([WorkerMessage::ExecuteSinks]).unwrap();
    let (reply, sync_rx) = oneshot::channel();
    worker
        .send_many([WorkerMessage::ExecuteSinks, WorkerMessage::Sync { reply }])
        .unwrap();

    // Sync barrier: fires after the commit covering everything above.
    timeout(Duration::from_millis(500), sync_rx)
        .await
        .expect("Sync timeout")
        .expect("Sync sender dropped");

    // Two ExecuteSinks across the drained batch reduce to one
    // idempotent flag → one execute → one callback.
    let first = compute_finish_rx
        .try_recv()
        .expect("batch must fire callback");
    assert!(first.is_ok(), "{first:?}");
    assert!(
        compute_finish_rx.try_recv().is_err(),
        "two ExecuteSinks across batches must reduce to one callback"
    );
    assert_eq!(messages(first.as_ref().unwrap()), ["once"]);
}

#[tokio::test]
async fn execute_sinks_with_start_event_loop_on_empty_graph_is_silent_noop() {
    // F5 + F4: a batch with ExecuteSinks + StartEventLoop on an
    // empty graph must fire no callback at all.
    let (worker, mut rx) = finished_worker(8);

    sync_after(
        &worker,
        [WorkerMessage::ExecuteSinks, WorkerMessage::StartEventLoop],
    )
    .await;
    assert_no_callback_within(&mut rx, Duration::from_millis(100)).await;
}

#[test]
fn scan_accumulates_simple_flags() {
    let (reply_ack, _ack_rx) = oneshot::channel();
    let node_id = NodeId::unique();
    let event = EventRef {
        node_id,
        event_idx: 0,
    };

    let intent = scan(vec![
        WorkerMessage::Clear,
        WorkerMessage::StartEventLoop,
        WorkerMessage::ExecuteSinks,
        WorkerMessage::InjectEvents {
            events: vec![event],
        },
        WorkerMessage::ExecuteNodes {
            nodes: vec![NodeAddress::root(node_id)],
        },
        WorkerMessage::ExecuteNodes {
            nodes: vec![NodeAddress::root(node_id)],
        },
        WorkerMessage::Sync { reply: reply_ack },
    ]);

    assert!(matches!(intent.graph_state, Some(GraphOp::Clear)));
    assert!(matches!(intent.loop_request, Some(LoopCommand::Start)));
    assert!(intent.execute_sinks);
    assert!(!intent.exit);
    assert_eq!(intent.events.values.len(), 1);
    assert!(intent.events.seen.contains(&event));
    assert_eq!(
        intent.execute_nodes.values.len(),
        1,
        "duplicate node seeds union to one"
    );
    assert!(
        intent
            .execute_nodes
            .seen
            .contains(&NodeAddress::root(node_id))
    );
    assert_eq!(intent.syncs.len(), 1);
}

#[test]
fn scan_deduplicates_events() {
    let node_id = NodeId::unique();
    let event = EventRef {
        node_id,
        event_idx: 0,
    };

    let intent = scan(vec![
        WorkerMessage::InjectEvents {
            events: vec![event],
        },
        WorkerMessage::InjectEvents {
            events: vec![event],
        },
        WorkerMessage::InjectEvents {
            events: vec![event, event],
        },
    ]);

    assert_eq!(
        intent.events.values.len(),
        1,
        "duplicate events must collapse to one"
    );
}

/// A trivially-valid `Update` payload for scan tests, which only inspect the
/// reduced intent — the program's content is irrelevant.
fn empty_compiled() -> CompiledGraph {
    Compiler::default()
        .compile(&Graph::default(), &Library::default())
        .unwrap()
}

#[test]
fn scan_exit_dominates_entire_batch() {
    // Exit is sticky across the whole batch: every other command
    // in the batch is discarded, whether sent before or after.
    let intent = scan(vec![
        WorkerMessage::Clear,
        WorkerMessage::ExecuteSinks,
        WorkerMessage::Exit,
        WorkerMessage::StartEventLoop, // post-Exit: dropped
        WorkerMessage::Update {
            compiled: empty_compiled(),
        },
    ]);

    assert!(intent.exit);
    assert!(
        intent.graph_state.is_none(),
        "pre-Exit graph ops must be discarded"
    );
    assert!(
        intent.loop_request.is_none(),
        "post-Exit loop ops must be discarded"
    );
    assert!(
        !intent.execute_sinks,
        "pre-Exit execute_sinks must be discarded"
    );
    assert!(intent.events.values.is_empty());
    assert!(intent.syncs.is_empty());
}

#[test]
fn scan_update_overwrites_earlier_update_in_same_batch() {
    // Two Updates in one batch: the last one wins. This is
    // implicit today (Option::replace) but worth pinning since
    // callers do send [Update(A), Update(B)] during rapid edits.
    let intent = scan(vec![
        WorkerMessage::Update {
            compiled: empty_compiled(),
        },
        WorkerMessage::Update {
            compiled: empty_compiled(),
        },
    ]);

    assert!(matches!(intent.graph_state, Some(GraphOp::Replace(_))));
}

// Slot reduction: last-write-wins for graph_state and loop_request.

#[test]
fn scan_last_write_wins_per_slot() {
    type Expect = fn(&BatchIntent) -> bool;
    let cases: [(&str, Vec<WorkerMessage>, Expect); 4] = [
        (
            "Clear then Update -> last write (Update) wins",
            vec![
                WorkerMessage::Clear,
                WorkerMessage::Update {
                    compiled: empty_compiled(),
                },
            ],
            |intent| matches!(intent.graph_state, Some(GraphOp::Replace(_))),
        ),
        (
            "Update then Clear -> last write (Clear) wins",
            vec![
                WorkerMessage::Update {
                    compiled: empty_compiled(),
                },
                WorkerMessage::Clear,
            ],
            |intent| matches!(intent.graph_state, Some(GraphOp::Clear)),
        ),
        (
            "Start then Stop -> last write (Stop) wins",
            vec![WorkerMessage::StartEventLoop, WorkerMessage::StopEventLoop],
            |intent| matches!(intent.loop_request, Some(LoopCommand::Stop)),
        ),
        (
            "Stop then Start -> last write (Start) wins",
            vec![WorkerMessage::StopEventLoop, WorkerMessage::StartEventLoop],
            |intent| matches!(intent.loop_request, Some(LoopCommand::Start)),
        ),
    ];

    for (label, msgs, expected) in cases {
        let intent = scan(msgs);
        assert!(expected(&intent), "{label}");
    }
}

// Integration: end-to-end confirmation that Update-then-Clear in
// one batch leaves the execution graph empty — a subsequent
// InjectEvents hits the empty-graph silent no-op path (no callback).
#[tokio::test]
async fn update_then_clear_in_same_batch_leaves_graph_cleared() {
    let mut h = FrameHarness::new().await;

    h.worker
        .send_many([h.update_msg(), WorkerMessage::Clear, h.inject_frame_event()])
        .unwrap();
    assert_no_callback_within(&mut h.compute_rx, Duration::from_millis(100)).await;
}

// Lambda fires as fast as it can; a Stop command must be observed
// and acted on within a bounded time rather than being delayed
// indefinitely by the event stream. Without `biased;` on the
// select!, this test would flake — the fair-random polling could
// sit on the events branch for many iterations.
#[tokio::test]
async fn commands_not_starved_by_fast_event_loop() {
    let mut h = FrameHarness::with_callback_capacity(512).await;

    sync_after(&h.worker, [h.update_msg(), WorkerMessage::StartEventLoop]).await;
    h.compute_rx.recv().await.unwrap().unwrap();
    timeout(Duration::from_millis(500), h.compute_rx.recv())
        .await
        .expect("event loop did not produce a callback")
        .expect("callback channel closed")
        .expect("event loop execution failed");

    // Drain accumulated callbacks so the channel isn't a
    // confounding factor.
    while h.compute_rx.try_recv().is_ok() {}

    // Send Stop + Sync. Both must be observed within the budget
    // even though lambda events are still being produced.
    sync_after(&h.worker, [WorkerMessage::StopEventLoop]).await;

    while h.compute_rx.try_recv().is_ok() {}
    assert_no_callback_within(&mut h.compute_rx, Duration::from_millis(100)).await;
}

// End-to-end: an event fired by a lambda reaches the worker's
// execute path and produces an execution_callback. Covers the
// dedicated bounded-channel flow (no forwarder).
#[tokio::test]
async fn lambda_events_drive_worker_execution() {
    let mut h = FrameHarness::with_callback_capacity(32).await;

    h.worker
        .send_many([h.update_msg(), WorkerMessage::StartEventLoop])
        .unwrap();

    // First callback is the initial execute() inside the
    // start_event_loop path.
    let _ = timeout(Duration::from_millis(500), h.compute_rx.recv())
        .await
        .expect("initial execute callback")
        .expect("callback channel closed");

    // Subsequent callbacks must come from the lambda firing and
    // the worker draining events from the bounded channel.
    let second = timeout(Duration::from_millis(500), h.compute_rx.recv())
        .await
        .expect("lambda-driven execute callback")
        .expect("callback channel closed");
    assert!(
        second.is_ok(),
        "lambda-driven execution must succeed: {second:?}"
    );
}

// F4: Exit dominates the batch at the runtime level — any pre-Exit
// Sync reply sender must drop (waiter observes RecvError), not fire
// with `Ok(())`. `scan_exit_dominates_entire_batch` pins the pure
// scan side; this pins the end-to-end contract.
#[tokio::test]
async fn exit_in_batch_closes_pending_sync() {
    let (worker, _rx) = finished_worker(8);

    let (reply, rx) = oneshot::channel();
    worker
        .send_many([WorkerMessage::Sync { reply }, WorkerMessage::Exit])
        .unwrap();

    let result = timeout(Duration::from_millis(500), rx)
        .await
        .expect("rx must resolve once the dropped Sync sender is observed");
    assert!(
        result.is_err(),
        "Sync batched with Exit must drop the reply sender, got {result:?}"
    );
}

// F6: StartEventLoop while a loop is already running must not trip
// the `assert!(event_loop.is_none())` in the commit phase — the
// needs_stop path is what keeps that invariant, and this test
// exists so a refactor touching needs_stop fails loudly.
#[tokio::test]
async fn start_event_loop_twice_is_idempotent() {
    let mut h = FrameHarness::with_callback_capacity(64).await;

    sync_after(&h.worker, [h.update_msg(), WorkerMessage::StartEventLoop]).await;
    while h.compute_rx.try_recv().is_ok() {}
    timeout(Duration::from_millis(500), h.compute_rx.recv())
        .await
        .expect("event loop did not produce a callback before restart")
        .expect("callback channel closed")
        .expect("event loop execution failed");

    // Second StartEventLoop — no graph change; must not panic.
    sync_after(&h.worker, [WorkerMessage::StartEventLoop]).await;
    while h.compute_rx.try_recv().is_ok() {}
    timeout(Duration::from_millis(500), h.compute_rx.recv())
        .await
        .expect("event loop did not produce a callback after restart")
        .expect("callback channel closed")
        .expect("event loop execution failed");
}

// F8: dropping a Worker without calling exit() must not leak or
// panic. Uses a shared flag to prove the tokio task actually
// terminates — if Drop regresses (e.g. thread_handle take but no
// abort), the lambda inside a running loop would keep running past
// the drop.
#[tokio::test]
async fn drop_without_exit_shuts_down_cleanly() {
    use std::sync::atomic::{AtomicUsize, Ordering};

    let counter = Arc::new(AtomicUsize::new(0));
    {
        let counter_cb = Arc::clone(&counter);
        let worker = Worker::new(move |_| {
            counter_cb.fetch_add(1, Ordering::SeqCst);
        });

        // Minimal traffic: confirm the worker is alive before drop.
        sync_after(&worker, std::iter::empty()).await;

        // `worker` goes out of scope → Drop → exit().
    }

    // Give any dangling task time to (incorrectly) run; none should.
    let before = counter.load(Ordering::SeqCst);
    tokio::time::sleep(Duration::from_millis(50)).await;
    let after = counter.load(Ordering::SeqCst);
    assert_eq!(
        before, after,
        "no callbacks must fire after Worker is dropped"
    );
}

/// The disk cache wires through both entry points and persists across worker
/// restarts: a `persist` (Disk-marked) reproducible node's output, stored on a
/// cold run, reloads on a fresh worker over the same store so its upstream never
/// recomputes. The cache is set at runtime via a `SetDiskStore` message in the
/// same batch as `Update` — exercising that it's applied before the compile
/// hydrates.
#[tokio::test]
async fn disk_cache_persists_node_across_worker_restart() {
    use std::path::Path;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use crate::execution::disk_store::DiskStore;
    use crate::graph::CacheMode;
    use crate::testing::{TestFuncHooks, test_func_lib};

    let dir = temp_dir("diskcache");

    // `get_a` recompute counter, shared across both worker incarnations.
    let get_a_calls = Arc::new(AtomicUsize::new(0));
    let make_lib = || {
        let calls = get_a_calls.clone();
        test_func_lib(TestFuncHooks {
            get_a: Arc::new(move || {
                calls.fetch_add(1, Ordering::SeqCst);
                Ok(7)
            }),
            get_b: Arc::new(move || 11),
            print: Arc::new(move |_| {}),
        })
    };

    // get_a (pure source) → mult (pure, persist Disk) → print (sink).
    let lib = make_lib();
    let mut graph = Graph::default();
    for name in ["get_a", "mult", "Print"] {
        let node: Node = lib.by_name(name).unwrap().into();
        graph.add(node);
    }
    let get_a_id = graph
        .find_by_name("get_a", NodeSearch::TopLevel)
        .unwrap()
        .id;
    let mult_id = graph.find_by_name("mult", NodeSearch::TopLevel).unwrap().id;
    let print_id = graph
        .find_by_name("Print", NodeSearch::TopLevel)
        .unwrap()
        .id;
    graph
        .find_mut(&mult_id, NodeSearch::TopLevel)
        .unwrap()
        .cache = CacheMode::Disk;
    graph.set_input_binding(InputPort::new(mult_id, 0), Binding::bind(get_a_id, 0));
    graph.set_input_binding(InputPort::new(mult_id, 1), Binding::bind(get_a_id, 0));
    graph.set_input_binding(InputPort::new(print_id, 0), Binding::bind(mult_id, 0));

    async fn run(root: &Path, graph: Graph, library: Arc<Library>) -> ExecutionStats {
        let (worker, mut rx) = finished_worker(4);
        // SetDiskStore shares the batch with Update, proving it's applied before
        // the install hydrates.
        let cache = DiskStore::new(Arc::new(Library::default()), Some(root.to_path_buf()));
        worker
            .send_many([
                WorkerMessage::SetDiskStore(cache),
                WorkerMessage::Update {
                    compiled: Compiler::default().compile(&graph, &library).unwrap(),
                },
                WorkerMessage::ExecuteSinks,
            ])
            .unwrap();
        rx.recv()
            .await
            .expect("worker reports Finished")
            .expect("run succeeds")
    }

    // Cold cache: all three nodes compute; mult is stored to disk.
    let stats = run(&dir.0, graph.clone(), Arc::new(make_lib())).await;
    assert_eq!(
        stats.executed_nodes.len(),
        3,
        "cold run computes every node"
    );
    assert_eq!(get_a_calls.load(Ordering::SeqCst), 1);

    // Reopen on a fresh worker over the same store: mult loads from disk and is reused. Its
    // input `get_a` feeds only the reused mult, which never reads it, so the pre-run cut
    // prunes `get_a` — the `Memory` source is not recomputed on reopen.
    let stats = run(&dir.0, graph.clone(), Arc::new(make_lib())).await;
    assert_eq!(
        get_a_calls.load(Ordering::SeqCst),
        1,
        "the cut prunes the Memory input feeding only a disk-cache hit"
    );
    assert!(
        !stats.executed_nodes.iter().any(|n| n.node_id == get_a_id),
        "get_a was cut, not recomputed"
    );
    assert!(
        stats.cached_nodes.contains(&mult_id),
        "mult is served from the disk cache"
    );
    assert!(
        !stats.executed_nodes.iter().any(|n| n.node_id == mult_id),
        "mult itself is not recomputed"
    );
}

/// `SetDiskStore` flushes resident disk-backed values into the just-attached
/// store: a `Both`-mode value computed while no store root existed (an unsaved
/// document) would otherwise be a RAM hit on every later run — which never
/// stores — and silently recompute on reopen.
#[tokio::test]
async fn set_disk_store_flushes_resident_disk_backed_values() {
    use crate::execution::disk_store::DiskStore;
    use crate::graph::CacheMode;
    use crate::testing::{TestFuncHooks, test_func_lib};

    let dir = temp_dir("storeswap");

    let lib = Arc::new(test_func_lib(TestFuncHooks {
        get_a: Arc::new(|| Ok(7)),
        get_b: Arc::new(|| 11),
        print: Arc::new(|_| {}),
    }));
    // get_a → mult (Both) → print.
    let mut graph = Graph::default();
    for name in ["get_a", "mult", "Print"] {
        let node: Node = lib.by_name(name).unwrap().into();
        graph.add(node);
    }
    let get_a_id = graph
        .find_by_name("get_a", NodeSearch::TopLevel)
        .unwrap()
        .id;
    let mult_id = graph.find_by_name("mult", NodeSearch::TopLevel).unwrap().id;
    let print_id = graph
        .find_by_name("Print", NodeSearch::TopLevel)
        .unwrap()
        .id;
    graph
        .find_mut(&mult_id, NodeSearch::TopLevel)
        .unwrap()
        .cache = CacheMode::Both;
    graph.set_input_binding(InputPort::new(mult_id, 0), Binding::bind(get_a_id, 0));
    graph.set_input_binding(InputPort::new(print_id, 0), Binding::bind(mult_id, 0));

    let (worker, mut rx) = finished_worker(4);

    // Run with NO store root: the Both value stays resident-only.
    worker
        .send_many([
            WorkerMessage::Update {
                compiled: Compiler::default().compile(&graph, &lib).unwrap(),
            },
            WorkerMessage::ExecuteSinks,
        ])
        .unwrap();
    rx.recv().await.unwrap().unwrap();
    assert_eq!(std::fs::read_dir(&dir.0).unwrap().count(), 0);

    // Attaching the store must flush the resident Both value as a blob.
    let store = DiskStore::new(Arc::new(Library::default()), Some(dir.0.clone()));
    worker.send(WorkerMessage::SetDiskStore(store)).unwrap();
    let (sync_tx, sync_rx) = oneshot::channel();
    worker.send(WorkerMessage::Sync { reply: sync_tx }).unwrap();
    sync_rx.await.unwrap();

    assert_eq!(
        std::fs::read_dir(&dir.0).unwrap().count(),
        1,
        "the resident Both-mode value was flushed into the new store"
    );
}
