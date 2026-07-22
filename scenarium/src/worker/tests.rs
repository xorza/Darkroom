use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use tokio::sync::{Notify, mpsc, oneshot};
use tokio::time::{Duration, timeout};

use crate::StaticValue;
use crate::elements::system_library::system_library;
use crate::elements::worker_events_library::worker_events_library;
use crate::execution::compile::{CompiledGraph, Compiler};
use crate::execution::identity::{ExecutionIdentityError, ExecutionNodeId, NodeAddress};
use crate::execution::report::RunPhase;
use crate::execution::stats::ExecutionStats;
use crate::execution::{Result as ExecResult, RunSeeds};
use crate::graph::{Binding, Graph, InputPort, Node, NodeId, NodeSearch};
use crate::library::Library;
use crate::node::event::EventLambda;
use crate::runtime::shared_any_state::SharedAnyState;

use crate::execution::event::EventTrigger;
use crate::execution::identity::ExecutionEventPort;
use crate::worker::Worker;
use crate::worker::batch::{BatchIntent, GraphOp, LoopCommand, scan};
use crate::worker::event_loop::ActiveEventLoop;
use crate::worker::pause_gate::PauseGate;
use crate::worker::protocol::{WorkerMessage, WorkerReport};

type TestResult<T = ()> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

fn root_execution_node(node_id: NodeId) -> ExecutionNodeId {
    ExecutionNodeId::from_node_id(node_id)
}

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
/// `ExecutionEventPort`).
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
                .unwrap()
                .into(),
        }
    }

    fn frame_event(&self) -> ExecutionEventPort {
        ExecutionEventPort {
            e_node_id: root_execution_node(self.frame_event_node_id),
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

    graph.set_input_binding(InputPort::new(frame_event_node_id, 0), Binding::from(1i64));
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
        Binding::from(StaticValue::String(message.to_string())),
    );
    (graph, print_node_id)
}

/// Start an event loop with a single lambda as its only trigger, on a fresh
/// `NodeId` — the shape most `start_event_loop` tests want when they only
/// care about one lambda's behavior.
async fn start_single_event_loop(
    lambda: EventLambda,
    pause_gate: PauseGate,
) -> (ActiveEventLoop, ExecutionNodeId) {
    let e_node_id = ExecutionNodeId::unique();
    let active = ActiveEventLoop::start(
        vec![EventTrigger {
            event: ExecutionEventPort {
                e_node_id,
                event_idx: 0,
            },
            lambda,
            state: SharedAnyState::default(),
        }],
        pause_gate,
    )
    .await;
    (active, e_node_id)
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

#[derive(Debug)]
struct FinishedRun {
    compiled: Arc<CompiledGraph>,
    result: ExecResult<ExecutionStats>,
}

async fn next_finished_run(rx: &mut mpsc::Receiver<WorkerReport>) -> FinishedRun {
    let mut compiled = None;
    loop {
        let report = timeout(Duration::from_secs(5), rx.recv())
            .await
            .expect("worker timed out")
            .expect("worker channel closed");
        match report {
            WorkerReport::Installed(installed) => compiled = Some(installed),
            WorkerReport::Cleared => compiled = None,
            WorkerReport::Finished(result) => {
                return FinishedRun {
                    compiled: compiled.expect("Finished arrived before Installed"),
                    result,
                };
            }
            WorkerReport::Progress(_) | WorkerReport::PinnedOutputs(_) => {}
        }
    }
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
async fn test_worker() -> TestResult {
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
    let (mut active, e_node_id) = start_single_event_loop(event_lambda, PauseGate::default()).await;

    let event = active
        .events
        .recv()
        .await
        .expect("Expected event loop event");
    assert_eq!(
        event,
        ExecutionEventPort {
            e_node_id,
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

    let (mut active, e_node_id) = start_single_event_loop(event_lambda, PauseGate::default()).await;

    notify_for_callback.notify_waiters();

    let event = timeout(Duration::from_millis(200), active.events.recv())
        .await
        .expect("Expected event")
        .expect("Event channel closed");
    assert_eq!(
        event,
        ExecutionEventPort {
            e_node_id,
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
    let (mut active, e_node_id) = start_single_event_loop(event_lambda, PauseGate::default()).await;

    // The lambda panics on its first invoke and never sends; its sole sender
    // drops, closing the channel. Awaiting that close ensures the panic has
    // landed before we stop.
    assert!(
        active.events.recv().await.is_none(),
        "panicking lambda should close the event channel without sending"
    );

    let panics = active.stop().await;
    assert_eq!(panics.len(), 1, "the lambda panic should be captured");
    assert_eq!(
        panics[0].e_node_id, e_node_id,
        "panic attributed to its node"
    );
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
                compiled: Compiler::default()
                    .compile(&graph, &library)
                    .unwrap()
                    .into(),
            },
            WorkerMessage::Run {
                seeds: RunSeeds::sinks(),
            },
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
    let compiled: Arc<CompiledGraph> = Compiler::default()
        .compile(&graph, &library)
        .unwrap()
        .into();
    let expected_compiled = Arc::clone(&compiled);
    worker
        .send_many([
            WorkerMessage::Update { compiled },
            WorkerMessage::Run {
                seeds: RunSeeds::sinks(),
            },
        ])
        .unwrap();

    // The single node's Started + Finished progress both arrive (mapped to its
    // authoring id) ahead of the sink `Finished` report.
    let mut started = 0;
    let mut node_finished = 0;
    let mut installed = None;
    loop {
        let report = timeout(Duration::from_secs(5), rx.recv())
            .await
            .expect("worker timed out")
            .expect("worker channel closed");
        match report {
            WorkerReport::Installed(compiled) => {
                assert!(installed.is_none(), "one update installed more than once");
                assert!(Arc::ptr_eq(&compiled, &expected_compiled));
                installed = Some(compiled);
            }
            WorkerReport::Progress(p) => {
                let compiled = installed
                    .as_ref()
                    .expect("Progress arrived before Installed");
                assert_eq!(
                    p.e_node_id,
                    root_execution_node(print_node_id),
                    "progress maps to the node"
                );
                assert_eq!(
                    compiled.authoring_address(p.e_node_id).unwrap(),
                    &NodeAddress::root(print_node_id),
                );
                match p.phase {
                    RunPhase::Started { .. } => started += 1,
                    RunPhase::Finished { .. } => node_finished += 1,
                }
            }
            WorkerReport::PinnedOutputs(_) => {
                assert!(
                    installed.is_some(),
                    "PinnedOutputs arrived before Installed"
                );
            }
            WorkerReport::Finished(result) => {
                assert!(installed.is_some(), "Finished arrived before Installed");
                assert_eq!(result.expect("compute ok").executed_nodes.len(), 1);
                break;
            }
            WorkerReport::Cleared => panic!("unexpected clear"),
        }
    }
    assert_eq!(started, 1, "one Started before the final report");
    assert_eq!(
        node_finished, 1,
        "one Finished progress before the final report"
    );

    worker.send(WorkerMessage::Clear).unwrap();
    let cleared = timeout(Duration::from_secs(5), rx.recv())
        .await
        .expect("worker timed out")
        .expect("worker channel closed");
    assert!(matches!(cleared, WorkerReport::Cleared));
}

#[tokio::test]
async fn installed_program_distinguishes_repeated_definition_instances() {
    use std::collections::HashSet;

    use crate::DataType;
    use crate::graph::NodeKind;
    use crate::graph::interface::{GraphId, GraphLink};
    use crate::node::definition::FuncOutput;
    use crate::testing::{TestFuncHooks, test_func_lib};

    let library = test_func_lib(TestFuncHooks {
        get_a: Arc::new(|| Ok(1)),
        get_b: Arc::new(|| 7),
        print: Arc::new(|_| {}),
    });
    let mut definition = Graph::new("Repeated").output(FuncOutput::new("Out", DataType::Int));
    let interior = definition.add(library.by_name("get_b").unwrap().into());
    let output = definition.add(Node::new(NodeKind::GraphOutput));
    definition.set_input_binding(InputPort::new(output, 0), Binding::bind(interior, 0));

    let definition_id = GraphId::unique();
    let mut graph = Graph::default();
    graph.insert_graph(definition_id, definition.clone());
    let instance_a = graph.add_graph_node(&definition, GraphLink::Local(definition_id));
    let instance_b = graph.add_graph_node(&definition, GraphLink::Local(definition_id));
    for instance in [instance_a, instance_b] {
        let print = graph.add(library.by_name("Print").unwrap().into());
        graph.set_input_binding(InputPort::new(print, 0), Binding::bind(instance, 0));
    }

    let (tx, mut rx) = mpsc::channel::<WorkerReport>(32);
    let worker = Worker::new(move |report| {
        tx.try_send(report).unwrap();
    });
    worker
        .send_many([
            WorkerMessage::Update {
                compiled: Compiler::default()
                    .compile(&graph, &library)
                    .unwrap()
                    .into(),
            },
            WorkerMessage::Run {
                seeds: RunSeeds::sinks(),
            },
        ])
        .unwrap();

    let finished = next_finished_run(&mut rx).await;
    let stats = finished.result.unwrap();
    let addresses: HashSet<_> = stats
        .executed_nodes
        .iter()
        .map(|stats| {
            finished
                .compiled
                .authoring_address(stats.e_node_id)
                .unwrap()
        })
        .filter(|address| address.node_id == interior)
        .cloned()
        .collect();
    assert_eq!(
        addresses,
        HashSet::from([
            NodeAddress {
                instances: vec![instance_a],
                node_id: interior,
            },
            NodeAddress {
                instances: vec![instance_b],
                node_id: interior,
            },
        ])
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn cancel_without_an_active_run_does_not_affect_the_next_run() {
    let library = system_library();
    let (graph, _print_node_id) = print_literal_graph(&library, "hi");

    let (worker, mut rx) = finished_worker(8);

    worker.request_cancel();
    worker
        .send_many([
            WorkerMessage::Update {
                compiled: Compiler::default()
                    .compile(&graph, &library)
                    .unwrap()
                    .into(),
            },
            WorkerMessage::Run {
                seeds: RunSeeds::sinks(),
            },
        ])
        .unwrap();

    let stats = timeout(Duration::from_secs(5), rx.recv())
        .await
        .expect("worker timed out")
        .expect("worker channel closed")
        .expect("compute ok");
    assert!(!stats.cancelled, "an idle cancel affected the next run");
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

/// Node seeds end-to-end: the run seed overrides a compiled disabled node and the
/// worker runs only its cone (the sink `Print` panics if reached).
#[tokio::test]
async fn execute_nodes_overrides_disabled_seed_and_runs_only_its_cone() {
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
    graph
        .find_mut(&sum_id, NodeSearch::TopLevel)
        .unwrap()
        .disabled = true;
    let get_a_id = graph
        .find_by_name("get_a", NodeSearch::TopLevel)
        .unwrap()
        .id;
    let get_b_id = graph
        .find_by_name("get_b", NodeSearch::TopLevel)
        .unwrap()
        .id;

    let (worker, mut rx) = finished_worker(8);
    worker
        .send_many([
            WorkerMessage::Update {
                compiled: Compiler::default()
                    .compile(&graph, &library)
                    .unwrap()
                    .into(),
            },
            WorkerMessage::Run {
                seeds: RunSeeds::nodes(vec![root_execution_node(sum_id)]),
            },
        ])
        .unwrap();

    let stats = timeout(Duration::from_secs(5), rx.recv())
        .await
        .expect("worker timed out")
        .expect("worker channel closed")
        .expect("run ok");
    let mut executed = stats
        .executed_nodes
        .iter()
        .map(|node| node.e_node_id)
        .collect::<Vec<_>>();
    executed.sort();
    let mut expected = vec![
        root_execution_node(get_a_id),
        root_execution_node(get_b_id),
        root_execution_node(sum_id),
    ];
    expected.sort();
    assert_eq!(executed, expected, "only the disabled sum's cone ran");
    assert!(
        graph.find(&sum_id, NodeSearch::TopLevel).unwrap().disabled,
        "execution does not mutate the authoring graph"
    );
}

/// A compiled disabled sink must not participate in an ordinary sink run. The default
/// hooks panic if any node executes.
#[tokio::test]
async fn disabled_sink_stays_out_of_sink_runs() {
    use crate::testing::{TestFuncHooks, test_func_lib, test_graph};

    let library = test_func_lib(TestFuncHooks::default());
    let mut graph = test_graph();
    let print_id = graph
        .find_by_name("Print", NodeSearch::TopLevel)
        .unwrap()
        .id;
    graph
        .find_mut(&print_id, NodeSearch::TopLevel)
        .unwrap()
        .disabled = true;

    let (worker, mut rx) = finished_worker(8);
    worker
        .send_many([
            WorkerMessage::Update {
                compiled: Compiler::default()
                    .compile(&graph, &library)
                    .unwrap()
                    .into(),
            },
            WorkerMessage::Run {
                seeds: RunSeeds::sinks(),
            },
        ])
        .unwrap();

    let stats = timeout(Duration::from_secs(5), rx.recv())
        .await
        .expect("worker timed out")
        .expect("worker channel closed")
        .expect("run ok");
    assert!(stats.executed_nodes.is_empty());
    assert!(stats.missing_inputs.is_empty());
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
    worker
        .send(WorkerMessage::Run {
            seeds: RunSeeds::sinks(),
        })
        .unwrap();
    assert_no_callback_within(&mut rx, Duration::from_millis(100)).await;
}

#[tokio::test]
async fn event_on_empty_graph_is_silent_noop() {
    let (worker, mut rx) = finished_worker(8);
    worker
        .send(WorkerMessage::InjectEvents {
            events: vec![ExecutionEventPort {
                e_node_id: ExecutionNodeId::unique(),
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
// (sink seeds or InjectEvents) and StartEventLoop, the commit
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
                compiled: Compiler::default()
                    .compile(&graph, &library)
                    .unwrap()
                    .into(),
            },
            WorkerMessage::Run {
                seeds: RunSeeds::sinks(),
            },
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

#[tokio::test(flavor = "current_thread")]
async fn queued_separate_install_and_run_commands_preserve_program_order() {
    let library = system_library();
    let (graph_a, print_a) = print_literal_graph(&library, "first");
    let (graph_b, print_b) = print_literal_graph(&library, "second");
    let (tx, mut rx) = mpsc::channel::<WorkerReport>(16);
    let worker = Worker::new(move |report| {
        tx.try_send(report).unwrap();
    });
    let compiled_a: Arc<CompiledGraph> = Compiler::default()
        .compile(&graph_a, &library)
        .unwrap()
        .into();
    let compiled_b: Arc<CompiledGraph> = Compiler::default()
        .compile(&graph_b, &library)
        .unwrap()
        .into();

    worker
        .send(WorkerMessage::Update {
            compiled: Arc::clone(&compiled_a),
        })
        .unwrap();
    worker
        .send(WorkerMessage::Run {
            seeds: RunSeeds::sinks(),
        })
        .unwrap();
    worker
        .send(WorkerMessage::Update {
            compiled: Arc::clone(&compiled_b),
        })
        .unwrap();
    worker
        .send(WorkerMessage::Run {
            seeds: RunSeeds::sinks(),
        })
        .unwrap();

    let first = next_finished_run(&mut rx).await;
    let second = next_finished_run(&mut rx).await;
    assert!(Arc::ptr_eq(&first.compiled, &compiled_a));
    assert!(Arc::ptr_eq(&second.compiled, &compiled_b));
    assert_eq!(
        first
            .compiled
            .authoring_address(root_execution_node(print_a))
            .unwrap(),
        &NodeAddress::root(print_a),
    );
    assert_eq!(
        second
            .compiled
            .authoring_address(root_execution_node(print_b))
            .unwrap(),
        &NodeAddress::root(print_b),
    );
    assert_eq!(messages(&first.result.unwrap()), ["first"]);
    assert_eq!(messages(&second.result.unwrap()), ["second"]);
}

#[tokio::test(flavor = "multi_thread")]
async fn replacement_queued_during_a_run_is_reported_after_the_running_program() {
    use std::sync::{Condvar, Mutex};

    use crate::testing::{TestFuncHooks, test_func_lib};

    let started = Arc::new(Notify::new());
    let release = Arc::new((Mutex::new(false), Condvar::new()));
    let library = test_func_lib(TestFuncHooks {
        get_a: {
            let started = Arc::clone(&started);
            let release = Arc::clone(&release);
            Arc::new(move || {
                started.notify_one();
                let (lock, wake) = &*release;
                let released = lock.lock().unwrap();
                drop(wake.wait_while(released, |released| !*released).unwrap());
                Ok(7)
            })
        },
        get_b: Arc::new(|| 11),
        print: Arc::new(|_| {}),
    });
    let mut running_graph = Graph::default();
    let source = running_graph.add(library.by_name("get_a").unwrap().into());
    let sink = running_graph.add(library.by_name("Print").unwrap().into());
    running_graph.set_input_binding(InputPort::new(sink, 0), Binding::bind(source, 0));
    let (replacement_graph, replacement_node) = print_literal_graph(&system_library(), "next");

    let (tx, mut rx) = mpsc::channel::<WorkerReport>(16);
    let worker = Worker::new(move |report| {
        tx.try_send(report).unwrap();
    });
    let running_compiled: Arc<CompiledGraph> = Compiler::default()
        .compile(&running_graph, &library)
        .unwrap()
        .into();
    worker
        .send_many([
            WorkerMessage::Update {
                compiled: Arc::clone(&running_compiled),
            },
            WorkerMessage::Run {
                seeds: RunSeeds::sinks(),
            },
        ])
        .unwrap();
    timeout(Duration::from_secs(5), started.notified())
        .await
        .expect("run did not start");

    let replacement_library = system_library();
    let replacement_compiled: Arc<CompiledGraph> = Compiler::default()
        .compile(&replacement_graph, &replacement_library)
        .unwrap()
        .into();
    worker
        .send(WorkerMessage::Update {
            compiled: Arc::clone(&replacement_compiled),
        })
        .unwrap();
    {
        let (lock, wake) = &*release;
        *lock.lock().unwrap() = true;
        wake.notify_one();
    }
    sync_after(&worker, std::iter::empty()).await;

    let finished = next_finished_run(&mut rx).await;
    assert!(Arc::ptr_eq(&finished.compiled, &running_compiled));
    assert!(finished.result.is_ok());
    assert_eq!(
        finished
            .compiled
            .authoring_address(root_execution_node(source))
            .unwrap(),
        &NodeAddress::root(source),
    );
    assert_eq!(
        finished
            .compiled
            .authoring_address(root_execution_node(sink))
            .unwrap(),
        &NodeAddress::root(sink),
    );
    let replacement_execution_node = root_execution_node(replacement_node);
    assert_eq!(
        finished
            .compiled
            .authoring_address(replacement_execution_node),
        Err(ExecutionIdentityError::NodeNotFound {
            e_node_id: replacement_execution_node,
        })
    );

    let installed = timeout(Duration::from_secs(5), rx.recv())
        .await
        .expect("worker timed out")
        .expect("worker channel closed");
    let WorkerReport::Installed(installed) = installed else {
        panic!("replacement installed before the running program finished");
    };
    assert!(Arc::ptr_eq(&installed, &replacement_compiled));
}

#[tokio::test]
async fn execute_sinks_with_start_event_loop_on_empty_graph_is_silent_noop() {
    // F5 + F4: a batch with sink seeds + StartEventLoop on an
    // empty graph must fire no callback at all.
    let (worker, mut rx) = finished_worker(8);

    sync_after(
        &worker,
        [
            WorkerMessage::Run {
                seeds: RunSeeds::sinks(),
            },
            WorkerMessage::StartEventLoop,
        ],
    )
    .await;
    assert_no_callback_within(&mut rx, Duration::from_millis(100)).await;
}

#[test]
fn scan_accumulates_simple_flags() {
    let (reply_ack, _ack_rx) = oneshot::channel();
    let e_node_id = ExecutionNodeId::unique();
    let event = ExecutionEventPort {
        e_node_id,
        event_idx: 0,
    };

    let intent = scan(vec![
        WorkerMessage::Clear,
        WorkerMessage::StartEventLoop,
        WorkerMessage::Run {
            seeds: RunSeeds::sinks(),
        },
        WorkerMessage::InjectEvents {
            events: vec![event],
        },
        WorkerMessage::Run {
            seeds: RunSeeds::nodes(vec![e_node_id]),
        },
        WorkerMessage::Run {
            seeds: RunSeeds::nodes(vec![e_node_id]),
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
    assert!(intent.execute_nodes.seen.contains(&e_node_id));
    assert_eq!(intent.syncs.len(), 1);
}

#[test]
fn scan_deduplicates_events() {
    let e_node_id = ExecutionNodeId::unique();
    let event = ExecutionEventPort {
        e_node_id,
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
fn empty_compiled() -> Arc<CompiledGraph> {
    Compiler::default()
        .compile(&Graph::default(), &Library::default())
        .unwrap()
        .into()
}

#[test]
fn scan_exit_dominates_entire_batch() {
    // Exit is sticky across the whole batch: every other command
    // in the batch is discarded, whether sent before or after.
    let intent = scan(vec![
        WorkerMessage::Clear,
        WorkerMessage::Run {
            seeds: RunSeeds::sinks(),
        },
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
                    compiled: Compiler::default()
                        .compile(&graph, &library)
                        .unwrap()
                        .into(),
                },
                WorkerMessage::Run {
                    seeds: RunSeeds::sinks(),
                },
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
        !stats
            .executed_nodes
            .iter()
            .any(|n| n.e_node_id == root_execution_node(get_a_id)),
        "get_a was cut, not recomputed"
    );
    assert!(
        stats.cached_nodes.contains(&root_execution_node(mult_id)),
        "mult is served from the disk cache"
    );
    assert!(
        !stats
            .executed_nodes
            .iter()
            .any(|n| n.e_node_id == root_execution_node(mult_id)),
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
                compiled: Compiler::default().compile(&graph, &lib).unwrap().into(),
            },
            WorkerMessage::Run {
                seeds: RunSeeds::sinks(),
            },
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
