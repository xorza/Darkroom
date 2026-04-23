use std::sync::Arc;

use common::output_stream::OutputStream;
use common::pause_gate::PauseGate;
use tokio::sync::{Notify, oneshot};
use tokio::time::{Duration, timeout};

use crate::common::shared_any_state::SharedAnyState;
use crate::elements::basic_funclib::BasicFuncLib;
use crate::elements::worker_events_funclib::WorkerEventsFuncLib;
use crate::event_lambda::EventLambda;
use crate::function::FuncLib;
use crate::graph::{Graph, Node, NodeId};

use crate::worker::{EventRef, EventTrigger, Worker, WorkerMessage};

fn log_frame_no_graph(func_lib: &FuncLib) -> Graph {
    let mut graph = Graph::default();

    let frame_event_node_id: NodeId = "e69c3f32-ac66-4447-a3f6-9e8528c5d830".into();
    let float_to_string_node_id: NodeId = "eb6590aa-229d-4874-abba-37c56f5b97fa".into();
    let print_node_id: NodeId = "8be72298-dece-4a5f-8a1d-d2dee1e791d3".into();

    let frame_event_func = func_lib.by_name("frame event").unwrap();
    let float_to_string_func = func_lib.by_name("float to string").unwrap();
    let print_func = func_lib.by_name("print").unwrap();

    let mut frame_event_node: Node = frame_event_func.into();
    frame_event_node.id = frame_event_node_id;
    frame_event_node.inputs[0].binding = 1.into();
    frame_event_node.events[0].subscribers.push(print_node_id);
    graph.add(frame_event_node);

    let mut float_to_string_node: Node = float_to_string_func.into();
    float_to_string_node.id = float_to_string_node_id;
    float_to_string_node.inputs[0].binding = (frame_event_node_id, 1).into();
    graph.add(float_to_string_node);

    let mut print_node: Node = print_func.into();
    print_node.id = print_node_id;
    print_node.inputs[0].binding = (float_to_string_node_id, 0).into();
    graph.add(print_node);

    graph
}

#[tokio::test]
async fn test_worker() -> anyhow::Result<()> {
    let output_stream = OutputStream::new();

    let timers_invoker = WorkerEventsFuncLib::default();
    let basic_invoker = BasicFuncLib::with_output_stream(&output_stream).await;

    let mut func_lib = basic_invoker.into_func_lib();
    func_lib.merge(timers_invoker.into_func_lib());

    let graph = log_frame_no_graph(&func_lib);
    let frame_event_node_id = graph.by_name("frame event").unwrap().id;

    let (compute_finish_tx, mut compute_finish_rx) = tokio::sync::mpsc::channel(8);
    let mut worker = Worker::new(move |result| {
        compute_finish_tx
            .try_send(result)
            .expect("Failed to send a compute callback event");
    });

    worker.send_many([
        WorkerMessage::Update {
            graph: graph.clone(),
            func_lib: Arc::new(func_lib.clone()),
        },
        WorkerMessage::InjectEvents {
            events: vec![EventRef {
                node_id: frame_event_node_id,
                event_idx: 0,
            }],
        },
    ]);

    let executed = compute_finish_rx
        .recv()
        .await
        .expect("Missing compute completion")
        .expect("Unsuccessful compute");

    assert_eq!(executed.executed_nodes.len(), 3);
    assert_eq!(output_stream.take().await, ["1"]);

    worker.send(WorkerMessage::InjectEvents {
        events: vec![EventRef {
            node_id: frame_event_node_id,
            event_idx: 0,
        }],
    });

    let executed = compute_finish_rx
        .recv()
        .await
        .expect("Missing compute completion")
        .expect("Unsuccessful compute");

    // frame_no is incremented in the update lambda, so each execution increments it
    assert_eq!(executed.executed_nodes.len(), 3);
    assert_eq!(output_stream.take().await, ["2"]);

    worker.send(WorkerMessage::InjectEvents {
        events: vec![EventRef {
            node_id: frame_event_node_id,
            event_idx: 0,
        }],
    });

    let executed = compute_finish_rx
        .recv()
        .await
        .expect("Missing compute completion")
        .expect("Unsuccessful compute");

    assert_eq!(executed.executed_nodes.len(), 3);
    assert_eq!(output_stream.take().await, ["3"]);

    worker.exit();

    Ok(())
}

#[tokio::test]
async fn start_event_loop_forwards_events() {
    let node_id = NodeId::unique();
    let event_lambda = EventLambda::new(|_state| Box::pin(async move {}));
    let event_state = SharedAnyState::default();

    let (mut handle, mut event_rx) = super::start_event_loop(
        vec![EventTrigger {
            event: EventRef {
                node_id,
                event_idx: 0,
            },
            lambda: event_lambda,
            state: event_state,
        }],
        PauseGate::default(),
    )
    .await;

    let event = event_rx.recv().await.expect("Expected event loop event");
    assert_eq!(
        event,
        EventRef {
            node_id,
            event_idx: 0
        }
    );

    handle.stop().await;
}

#[tokio::test]
async fn start_event_loop_waits_for_callback() {
    let node_id = NodeId::unique();
    let notify = Arc::new(Notify::new());
    let notify_for_event = Arc::clone(&notify);
    let event_lambda = EventLambda::new(move |_state| {
        let notify = Arc::clone(&notify_for_event);
        Box::pin(async move {
            notify.notified().await;
        })
    });
    let event_state = SharedAnyState::default();

    let notify_for_callback = Arc::clone(&notify);

    let (mut handle, mut event_rx) = super::start_event_loop(
        vec![EventTrigger {
            event: EventRef {
                node_id,
                event_idx: 0,
            },
            lambda: event_lambda,
            state: event_state,
        }],
        PauseGate::default(),
    )
    .await;

    notify_for_callback.notify_waiters();

    let event = timeout(Duration::from_millis(200), event_rx.recv())
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

    handle.stop().await;
}

#[tokio::test]
async fn pause_gate_blocks_event_loop_iterations() {
    use std::sync::atomic::{AtomicUsize, Ordering};

    let node_id = NodeId::unique();
    let invoke_count = Arc::new(AtomicUsize::new(0));
    let invoke_count_clone = Arc::clone(&invoke_count);

    let event_lambda = EventLambda::new(move |_state| {
        let invoke_count = Arc::clone(&invoke_count_clone);
        Box::pin(async move {
            invoke_count.fetch_add(1, Ordering::SeqCst);
        })
    });
    let event_state = SharedAnyState::default();

    let pause_gate = PauseGate::default();

    let (mut handle, mut event_rx) = super::start_event_loop(
        vec![EventTrigger {
            event: EventRef {
                node_id,
                event_idx: 0,
            },
            lambda: event_lambda,
            state: event_state,
        }],
        pause_gate.clone(),
    )
    .await;

    // Wait for first event to arrive
    let _ = timeout(Duration::from_millis(100), event_rx.recv())
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

    handle.stop().await;
}

#[tokio::test]
async fn clear_resets_execution_graph() {
    let output_stream = OutputStream::new();

    let timers_invoker = WorkerEventsFuncLib::default();
    let basic_invoker = BasicFuncLib::with_output_stream(&output_stream).await;

    let mut func_lib = basic_invoker.into_func_lib();
    func_lib.merge(timers_invoker.into_func_lib());

    let graph = log_frame_no_graph(&func_lib);
    let frame_event_node_id = graph.by_name("frame event").unwrap().id;

    let (compute_finish_tx, mut compute_finish_rx) = tokio::sync::mpsc::channel(8);
    let mut worker = Worker::new(move |result| {
        compute_finish_tx.try_send(result).ok();
    });

    // Setup and execute once
    worker.send_many([
        WorkerMessage::Update {
            graph: graph.clone(),
            func_lib: Arc::new(func_lib.clone()),
        },
        WorkerMessage::InjectEvents {
            events: vec![EventRef {
                node_id: frame_event_node_id,
                event_idx: 0,
            }],
        },
    ]);

    let _ = compute_finish_rx.recv().await;
    assert_eq!(output_stream.take().await, ["1"]);

    // Clear the graph
    worker.send(WorkerMessage::Clear);

    // Try to execute - should not produce output since graph is cleared
    worker.send(WorkerMessage::InjectEvents {
        events: vec![EventRef {
            node_id: frame_event_node_id,
            event_idx: 0,
        }],
    });

    // Give it time to process - no callback expected since graph is clear
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(output_stream.take().await.is_empty());

    worker.exit();
}

#[tokio::test]
async fn events_are_deduplicated() {
    let output_stream = OutputStream::new();

    let timers_invoker = WorkerEventsFuncLib::default();
    let basic_invoker = BasicFuncLib::with_output_stream(&output_stream).await;

    let mut func_lib = basic_invoker.into_func_lib();
    func_lib.merge(timers_invoker.into_func_lib());

    let graph = log_frame_no_graph(&func_lib);
    let frame_event_node_id = graph.by_name("frame event").unwrap().id;

    let (compute_finish_tx, mut compute_finish_rx) = tokio::sync::mpsc::channel(8);
    let mut worker = Worker::new(move |result| {
        compute_finish_tx.try_send(result).ok();
    });

    worker.send(WorkerMessage::Update {
        graph: graph.clone(),
        func_lib: Arc::new(func_lib.clone()),
    });

    // Send the same event multiple times in one batch
    let event = EventRef {
        node_id: frame_event_node_id,
        event_idx: 0,
    };
    worker.send(WorkerMessage::InjectEvents {
        events: vec![event, event, event],
    });

    let _ = compute_finish_rx.recv().await;

    // Should only print once since duplicate events are deduplicated
    assert_eq!(output_stream.take().await, ["1"]);

    worker.exit();
}

#[tokio::test]
async fn execute_terminals_triggers_terminal_nodes() {
    use crate::data::StaticValue;

    let output_stream = OutputStream::new();

    let basic_invoker = BasicFuncLib::with_output_stream(&output_stream).await;
    let func_lib = basic_invoker.into_func_lib();

    // Create a simple graph with a terminal print node
    let mut graph = Graph::default();
    let print_func = func_lib.by_name("print").unwrap();

    let mut print_node: Node = print_func.into();
    print_node.id = NodeId::unique();
    // print function is already terminal by definition
    print_node.inputs[0].binding = StaticValue::String("hello".to_string()).into();
    graph.add(print_node);

    let (compute_finish_tx, mut compute_finish_rx) = tokio::sync::mpsc::channel(8);
    let mut worker = Worker::new(move |result| {
        compute_finish_tx.try_send(result).ok();
    });

    worker.send_many([
        WorkerMessage::Update {
            graph: graph.clone(),
            func_lib: Arc::new(func_lib.clone()),
        },
        WorkerMessage::ExecuteTerminals,
    ]);

    let executed = compute_finish_rx
        .recv()
        .await
        .expect("Missing compute completion")
        .expect("Unsuccessful compute");

    assert_eq!(executed.executed_nodes.len(), 1);
    assert_eq!(output_stream.take().await, ["hello"]);

    worker.exit();
}

#[tokio::test]
async fn start_stop_event_loop() {
    let output_stream = OutputStream::new();

    let timers_invoker = WorkerEventsFuncLib::default();
    let basic_invoker = BasicFuncLib::with_output_stream(&output_stream).await;

    let mut func_lib = basic_invoker.into_func_lib();
    func_lib.merge(timers_invoker.into_func_lib());

    let graph = log_frame_no_graph(&func_lib);

    let (compute_finish_tx, mut compute_finish_rx) = tokio::sync::mpsc::channel(32);
    let mut worker = Worker::new(move |result| {
        compute_finish_tx.try_send(result).ok();
    });

    worker.send_many([
        WorkerMessage::Update {
            graph: graph.clone(),
            func_lib: Arc::new(func_lib.clone()),
        },
        WorkerMessage::StartEventLoop,
    ]);

    // Wait for event loop to start and produce some output
    let _ = compute_finish_rx.recv().await; // Initial execution
    tokio::time::sleep(Duration::from_millis(100)).await;

    assert!(worker.is_event_loop_started());

    // Stop the event loop
    worker.send(WorkerMessage::StopEventLoop);
    tokio::time::sleep(Duration::from_millis(50)).await;

    assert!(!worker.is_event_loop_started());

    worker.exit();
}

#[tokio::test]
async fn request_argument_values_invokes_callback() {
    let timers_invoker = WorkerEventsFuncLib::default();
    let basic_invoker = BasicFuncLib::default();

    let mut func_lib = basic_invoker.into_func_lib();
    func_lib.merge(timers_invoker.into_func_lib());

    let graph = log_frame_no_graph(&func_lib);
    let frame_event_node_id = graph.by_name("frame event").unwrap().id;

    let (compute_finish_tx, mut compute_finish_rx) = tokio::sync::mpsc::channel(8);
    let mut worker = Worker::new(move |result| {
        compute_finish_tx.try_send(result).ok();
    });

    worker.send_many([
        WorkerMessage::Update {
            graph: graph.clone(),
            func_lib: Arc::new(func_lib.clone()),
        },
        WorkerMessage::InjectEvents {
            events: vec![EventRef {
                node_id: frame_event_node_id,
                event_idx: 0,
            }],
        },
    ]);

    // Wait for execution
    let _ = compute_finish_rx.recv().await;

    let (reply, rx) = oneshot::channel();
    worker.send(WorkerMessage::RequestArgumentValues {
        node_id: frame_event_node_id,
        reply,
    });

    let values = timeout(Duration::from_millis(200), rx)
        .await
        .expect("Reply timeout")
        .expect("Reply sender dropped");

    assert!(values.is_some());

    worker.exit();
}

#[tokio::test]
async fn sync_fires_after_execution() {
    let timers_invoker = WorkerEventsFuncLib::default();
    let basic_invoker = BasicFuncLib::default();

    let mut func_lib = basic_invoker.into_func_lib();
    func_lib.merge(timers_invoker.into_func_lib());

    let graph = log_frame_no_graph(&func_lib);
    let frame_event_node_id = graph.by_name("frame event").unwrap().id;

    let (compute_finish_tx, mut compute_finish_rx) = tokio::sync::mpsc::channel(8);
    let mut worker = Worker::new(move |result| {
        compute_finish_tx.try_send(result).ok();
    });

    let (reply, rx) = oneshot::channel();
    worker.send_many([
        WorkerMessage::Update {
            graph: graph.clone(),
            func_lib: Arc::new(func_lib.clone()),
        },
        WorkerMessage::InjectEvents {
            events: vec![EventRef {
                node_id: frame_event_node_id,
                event_idx: 0,
            }],
        },
        WorkerMessage::Sync { reply },
    ]);

    let _ = compute_finish_rx.recv().await;
    timeout(Duration::from_millis(200), rx)
        .await
        .expect("Sync timeout")
        .expect("Sync sender dropped");

    worker.exit();
}

#[tokio::test]
async fn update_restarts_event_loop_if_running() {
    let output_stream = OutputStream::new();

    let timers_invoker = WorkerEventsFuncLib::default();
    let basic_invoker = BasicFuncLib::with_output_stream(&output_stream).await;

    let mut func_lib = basic_invoker.into_func_lib();
    func_lib.merge(timers_invoker.into_func_lib());

    let graph = log_frame_no_graph(&func_lib);

    let (compute_finish_tx, mut compute_finish_rx) = tokio::sync::mpsc::channel(32);
    let mut worker = Worker::new(move |result| {
        compute_finish_tx.try_send(result).ok();
    });

    // Start with event loop
    worker.send_many([
        WorkerMessage::Update {
            graph: graph.clone(),
            func_lib: Arc::new(func_lib.clone()),
        },
        WorkerMessage::StartEventLoop,
    ]);

    let _ = compute_finish_rx.recv().await;
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(worker.is_event_loop_started());

    // Update graph - should restart event loop
    worker.send(WorkerMessage::Update {
        graph: graph.clone(),
        func_lib: Arc::new(func_lib.clone()),
    });

    // Drain the channel
    while compute_finish_rx.try_recv().is_ok() {}

    tokio::time::sleep(Duration::from_millis(100)).await;

    // Event loop should still be running after update
    assert!(worker.is_event_loop_started());

    worker.exit();
}

// Stale-event filtering is now structural: each start_event_loop
// call returns a fresh Receiver; stop_event_loop drops the old
// pair so any undelivered events die with the channel. This test
// verifies the structural guarantee by confirming the old
// Receiver is closed after its sibling handle is stopped.
#[tokio::test]
async fn stopped_event_loop_channel_is_closed() {
    let node_id = NodeId::unique();
    let event_lambda = EventLambda::new(|_state| Box::pin(async move {}));
    let event_state = SharedAnyState::default();

    let (mut handle, mut event_rx) = super::start_event_loop(
        vec![EventTrigger {
            event: EventRef {
                node_id,
                event_idx: 0,
            },
            lambda: event_lambda,
            state: event_state,
        }],
        PauseGate::default(),
    )
    .await;

    handle.stop().await;

    // After stop, all lambda tasks (the sole senders) are aborted
    // → the Receiver must observe channel closure on next recv().
    let closed = timeout(Duration::from_millis(500), event_rx.recv())
        .await
        .expect("recv should complete once channel closes");
    // recv_many / recv return None when all senders are dropped.
    // Buffered events may still be present (fine); what matters
    // is that eventually we get None — drain and confirm.
    if closed.is_some() {
        while event_rx.recv().await.is_some() {}
    }
}

#[tokio::test]
async fn send_many_empty_is_noop() {
    // Empty batch must not panic, hang, or desynchronize the worker.
    let mut worker = Worker::new(|_| {});

    worker.send_many(std::iter::empty::<WorkerMessage>());

    // Subsequent Sync still fires → worker is alive.
    let (reply, rx) = oneshot::channel();
    worker.send(WorkerMessage::Sync { reply });
    timeout(Duration::from_millis(500), rx)
        .await
        .expect("Sync should fire after empty send_many")
        .expect("Sync sender dropped");

    worker.exit();
}

#[tokio::test]
async fn stop_event_loop_when_not_running_is_noop() {
    let mut worker = Worker::new(|_| {});

    worker.send(WorkerMessage::StopEventLoop);
    assert!(!worker.is_event_loop_started());

    // Worker still responsive after a no-op stop.
    let (reply, rx) = oneshot::channel();
    worker.send(WorkerMessage::Sync { reply });
    timeout(Duration::from_millis(500), rx)
        .await
        .expect("StopEventLoop with no running loop should be a no-op")
        .expect("Sync sender dropped");

    worker.exit();
}

#[tokio::test]
async fn request_argument_values_for_unknown_node_returns_none() {
    let timers_invoker = WorkerEventsFuncLib::default();
    let basic_invoker = BasicFuncLib::default();
    let mut func_lib = basic_invoker.into_func_lib();
    func_lib.merge(timers_invoker.into_func_lib());

    let graph = log_frame_no_graph(&func_lib);

    let (compute_finish_tx, _compute_finish_rx) = tokio::sync::mpsc::channel(8);
    let mut worker = Worker::new(move |result| {
        compute_finish_tx.try_send(result).ok();
    });

    worker.send(WorkerMessage::Update {
        graph,
        func_lib: Arc::new(func_lib),
    });

    let (reply, rx) = oneshot::channel();
    worker.send(WorkerMessage::RequestArgumentValues {
        node_id: NodeId::unique(),
        reply,
    });

    let values = timeout(Duration::from_millis(500), rx)
        .await
        .expect("Reply timeout")
        .expect("Reply sender dropped");

    assert!(values.is_none(), "unknown node should yield None");

    worker.exit();
}

#[tokio::test]
async fn multiple_syncs_in_batch_all_run() {
    let mut worker = Worker::new(|_| {});

    let (reply_a, rx_a) = oneshot::channel();
    let (reply_b, rx_b) = oneshot::channel();

    worker.send_many([
        WorkerMessage::Sync { reply: reply_a },
        WorkerMessage::Sync { reply: reply_b },
    ]);

    timeout(Duration::from_millis(500), rx_a)
        .await
        .expect("First Sync should fire")
        .expect("First sender dropped");
    timeout(Duration::from_millis(500), rx_b)
        .await
        .expect("Second Sync should fire")
        .expect("Second sender dropped");

    worker.exit();
}

#[tokio::test]
async fn clear_then_update_in_same_batch_applies_update() {
    // Scan-then-commit ordering: Clear zeroes execution_graph, Update
    // queues a replacement, commit phase applies Update and flips
    // execution_graph_clear back to false. The event must execute.
    let output_stream = OutputStream::new();
    let timers_invoker = WorkerEventsFuncLib::default();
    let basic_invoker = BasicFuncLib::with_output_stream(&output_stream).await;
    let mut func_lib = basic_invoker.into_func_lib();
    func_lib.merge(timers_invoker.into_func_lib());

    let graph = log_frame_no_graph(&func_lib);
    let frame_event_node_id = graph.by_name("frame event").unwrap().id;

    let (compute_finish_tx, mut compute_finish_rx) = tokio::sync::mpsc::channel(8);
    let mut worker = Worker::new(move |result| {
        compute_finish_tx.try_send(result).ok();
    });

    worker.send_many([
        WorkerMessage::Clear,
        WorkerMessage::Update {
            graph,
            func_lib: Arc::new(func_lib),
        },
        WorkerMessage::InjectEvents {
            events: vec![EventRef {
                node_id: frame_event_node_id,
                event_idx: 0,
            }],
        },
    ]);

    let executed = compute_finish_rx
        .recv()
        .await
        .expect("Missing compute completion")
        .expect("Unsuccessful compute");

    assert_eq!(executed.executed_nodes.len(), 3);
    assert_eq!(output_stream.take().await, ["1"]);

    worker.exit();
}

// F3: RequestArgumentValues batched with Update must observe the
// post-Update graph. Before the fix this ran inline during the scan
// phase and returned `None` because the Update was still pending in
// `BatchIntent`; after the fix it runs in commit phase and returns
// `Some`.
#[tokio::test]
async fn request_argument_values_batched_with_update_sees_new_graph() {
    let timers_invoker = WorkerEventsFuncLib::default();
    let basic_invoker = BasicFuncLib::default();
    let mut func_lib = basic_invoker.into_func_lib();
    func_lib.merge(timers_invoker.into_func_lib());

    let graph = log_frame_no_graph(&func_lib);
    let frame_event_node_id = graph.by_name("frame event").unwrap().id;

    let (compute_finish_tx, _rx) = tokio::sync::mpsc::channel(8);
    let mut worker = Worker::new(move |result| {
        compute_finish_tx.try_send(result).ok();
    });

    // Worker starts with an empty ExecutionGraph. Batch Update +
    // RequestArgumentValues together: only works if the scan phase
    // defers the request until after Update commits.
    let (reply, rx) = oneshot::channel();
    worker.send_many([
        WorkerMessage::Update {
            graph,
            func_lib: Arc::new(func_lib),
        },
        WorkerMessage::RequestArgumentValues {
            node_id: frame_event_node_id,
            reply,
        },
    ]);

    let values = timeout(Duration::from_millis(500), rx)
        .await
        .expect("Reply timeout")
        .expect("Reply sender dropped");

    assert!(
        values.is_some(),
        "request batched with Update must see the newly-loaded graph"
    );

    worker.exit();
}

// F3: Clear batched after an Update+Execute must clear the graph
// before a same-batch RequestArgumentValues runs → request returns
// None.
#[tokio::test]
async fn request_argument_values_batched_with_clear_sees_empty_graph() {
    let timers_invoker = WorkerEventsFuncLib::default();
    let basic_invoker = BasicFuncLib::default();
    let mut func_lib = basic_invoker.into_func_lib();
    func_lib.merge(timers_invoker.into_func_lib());

    let graph = log_frame_no_graph(&func_lib);
    let frame_event_node_id = graph.by_name("frame event").unwrap().id;

    let (compute_finish_tx, mut compute_finish_rx) = tokio::sync::mpsc::channel(8);
    let mut worker = Worker::new(move |result| {
        compute_finish_tx.try_send(result).ok();
    });

    // First populate the graph.
    worker.send(WorkerMessage::Update {
        graph,
        func_lib: Arc::new(func_lib),
    });
    // Sanity: prior request sees values.
    let (reply_before, rx_before) = oneshot::channel();
    worker.send(WorkerMessage::RequestArgumentValues {
        node_id: frame_event_node_id,
        reply: reply_before,
    });
    assert!(
        timeout(Duration::from_millis(500), rx_before)
            .await
            .expect("pre-clear reply timeout")
            .expect("pre-clear sender dropped")
            .is_some()
    );

    // Now batch Clear + RequestArgumentValues. Request must see the
    // post-clear state (None), not the pre-clear state (Some).
    let (reply_after, rx_after) = oneshot::channel();
    worker.send_many([
        WorkerMessage::Clear,
        WorkerMessage::RequestArgumentValues {
            node_id: frame_event_node_id,
            reply: reply_after,
        },
    ]);
    let values = timeout(Duration::from_millis(500), rx_after)
        .await
        .expect("post-clear reply timeout")
        .expect("post-clear sender dropped");
    assert!(
        values.is_none(),
        "request batched after Clear must observe empty graph"
    );

    // Drain any execution-finished callbacks the test produced.
    while compute_finish_rx.try_recv().is_ok() {}

    worker.exit();
}

// F5: running execution on an empty graph is a normal state, not a
// failure — the worker must skip execute silently. No callback fires
// until a graph is loaded.

async fn assert_no_callback_within(
    rx: &mut tokio::sync::mpsc::Receiver<
        crate::execution_graph::Result<crate::execution_stats::ExecutionStats>,
    >,
    d: Duration,
) {
    assert!(
        timeout(d, rx.recv()).await.is_err(),
        "empty-graph commands must be silent no-ops, but a callback fired"
    );
}

#[tokio::test]
async fn execute_terminals_on_empty_graph_is_silent_noop() {
    let (compute_finish_tx, mut compute_finish_rx) = tokio::sync::mpsc::channel(8);
    let mut worker = Worker::new(move |result| {
        compute_finish_tx.try_send(result).ok();
    });

    worker.send(WorkerMessage::ExecuteTerminals);
    assert_no_callback_within(&mut compute_finish_rx, Duration::from_millis(100)).await;

    worker.exit();
}

#[tokio::test]
async fn event_on_empty_graph_is_silent_noop() {
    let (compute_finish_tx, mut compute_finish_rx) = tokio::sync::mpsc::channel(8);
    let mut worker = Worker::new(move |result| {
        compute_finish_tx.try_send(result).ok();
    });

    worker.send(WorkerMessage::InjectEvents {
        events: vec![EventRef {
            node_id: NodeId::unique(),
            event_idx: 0,
        }],
    });
    assert_no_callback_within(&mut compute_finish_rx, Duration::from_millis(100)).await;

    worker.exit();
}

#[tokio::test]
async fn start_event_loop_on_empty_graph_is_silent_noop() {
    let (compute_finish_tx, mut compute_finish_rx) = tokio::sync::mpsc::channel(8);
    let mut worker = Worker::new(move |result| {
        compute_finish_tx.try_send(result).ok();
    });

    worker.send(WorkerMessage::StartEventLoop);
    assert_no_callback_within(&mut compute_finish_rx, Duration::from_millis(100)).await;
    assert!(
        !worker.is_event_loop_started(),
        "no loop should actually have started"
    );

    worker.exit();
}

// F4 regression: when a batch triggers both an execution
// (ExecuteTerminals or InjectEvents) and StartEventLoop, the commit
// phase must run execute() once and fire the callback once — not twice.

#[tokio::test]
async fn execute_terminals_with_start_event_loop_fires_callback_once() {
    use crate::data::StaticValue;

    // Terminal-only graph: active_event_triggers() returns empty, so
    // the loop never actually spawns. This removes lambda-driven
    // callbacks as a confounding factor while still exercising the
    // should_start_event_loop branch.
    let output_stream = OutputStream::new();
    let basic_invoker = BasicFuncLib::with_output_stream(&output_stream).await;
    let func_lib = basic_invoker.into_func_lib();

    let mut graph = Graph::default();
    let print_func = func_lib.by_name("print").unwrap();
    let mut print_node: Node = print_func.into();
    print_node.id = NodeId::unique();
    print_node.inputs[0].binding = StaticValue::String("hi".to_string()).into();
    graph.add(print_node);

    let (compute_finish_tx, mut compute_finish_rx) = tokio::sync::mpsc::channel(8);
    let mut worker = Worker::new(move |result| {
        compute_finish_tx.try_send(result).ok();
    });

    worker.send_many([
        WorkerMessage::Update {
            graph,
            func_lib: Arc::new(func_lib),
        },
        WorkerMessage::ExecuteTerminals,
        WorkerMessage::StartEventLoop,
    ]);

    let first = timeout(Duration::from_millis(500), compute_finish_rx.recv())
        .await
        .expect("batch must fire callback")
        .expect("callback channel closed");
    assert!(first.is_ok(), "execute must succeed: {first:?}");

    // Confirm no second callback for the same batch.
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(
        compute_finish_rx.try_recv().is_err(),
        "ExecuteTerminals+StartEventLoop must fire callback exactly once"
    );
    assert!(
        !worker.is_event_loop_started(),
        "terminal-only graph yields no triggers; loop should not have started"
    );
    // The single execute call actually ran the terminal.
    assert_eq!(output_stream.take().await, ["hi"]);

    worker.exit();
}

// F2: when two separate send_many calls queue messages before the
// worker wakes, the worker's drain-on-wake pulls both into a single
// BatchIntent — reducing per-slot. Under current_thread scheduling,
// the test task's synchronous sends all land in the channel before
// the worker task is polled, so this tests the drain path
// deterministically.
#[tokio::test(flavor = "current_thread")]
async fn drain_on_wake_folds_queued_batches_into_one_commit() {
    use crate::data::StaticValue;

    let output_stream = OutputStream::new();
    let basic_invoker = BasicFuncLib::with_output_stream(&output_stream).await;
    let func_lib = basic_invoker.into_func_lib();

    // Terminal-only graph — one execute produces one line of output.
    let mut graph = Graph::default();
    let print_func = func_lib.by_name("print").unwrap();
    let mut print_node: Node = print_func.into();
    print_node.id = NodeId::unique();
    print_node.inputs[0].binding = StaticValue::String("once".to_string()).into();
    graph.add(print_node);

    let (compute_finish_tx, mut compute_finish_rx) = tokio::sync::mpsc::channel(8);
    let mut worker = Worker::new(move |result| {
        compute_finish_tx.try_send(result).ok();
    });

    // Three separate send_many calls, all synchronous — they all
    // land in the channel before the worker task is polled.
    worker.send_many([WorkerMessage::Update {
        graph,
        func_lib: Arc::new(func_lib),
    }]);
    worker.send_many([WorkerMessage::ExecuteTerminals]);
    let (reply, sync_rx) = oneshot::channel();
    worker.send_many([
        WorkerMessage::ExecuteTerminals,
        WorkerMessage::Sync { reply },
    ]);

    // Sync barrier: fires after the commit covering everything above.
    timeout(Duration::from_millis(500), sync_rx)
        .await
        .expect("Sync timeout")
        .expect("Sync sender dropped");

    // Two ExecuteTerminals across the drained batch reduce to one
    // idempotent flag → one execute → one callback.
    let first = compute_finish_rx
        .try_recv()
        .expect("batch must fire callback");
    assert!(first.is_ok(), "{first:?}");
    assert!(
        compute_finish_rx.try_recv().is_err(),
        "two ExecuteTerminals across batches must reduce to one callback"
    );
    // Graph ran exactly once.
    assert_eq!(output_stream.take().await, ["once"]);

    worker.exit();
}

#[tokio::test]
async fn execute_terminals_with_start_event_loop_on_empty_graph_is_silent_noop() {
    // F5 + F4: a batch with ExecuteTerminals + StartEventLoop on an
    // empty graph must fire no callback at all.
    let (compute_finish_tx, mut compute_finish_rx) = tokio::sync::mpsc::channel(8);
    let mut worker = Worker::new(move |result| {
        compute_finish_tx.try_send(result).ok();
    });

    worker.send_many([
        WorkerMessage::ExecuteTerminals,
        WorkerMessage::StartEventLoop,
    ]);
    assert_no_callback_within(&mut compute_finish_rx, Duration::from_millis(100)).await;
    assert!(!worker.is_event_loop_started());

    worker.exit();
}

// --- Pure scan() unit tests ------------------------------------

#[test]
fn scan_accumulates_simple_flags() {
    let (reply_ack, _ack_rx) = oneshot::channel();
    let (reply_args, _args_rx) = oneshot::channel();
    let node_id = NodeId::unique();
    let event = EventRef {
        node_id,
        event_idx: 0,
    };

    let intent = super::scan(vec![
        WorkerMessage::Clear,
        WorkerMessage::StartEventLoop,
        WorkerMessage::ExecuteTerminals,
        WorkerMessage::InjectEvents {
            events: vec![event],
        },
        WorkerMessage::Sync { reply: reply_ack },
        WorkerMessage::RequestArgumentValues {
            node_id,
            reply: reply_args,
        },
    ]);

    assert!(matches!(intent.graph_state, Some(super::GraphOp::Clear)));
    assert!(matches!(
        intent.loop_request,
        Some(super::LoopCommand::Start)
    ));
    assert!(intent.execute_terminals);
    assert!(!intent.exit);
    assert_eq!(intent.events.len(), 1);
    assert!(intent.events.contains(&event));
    assert_eq!(intent.syncs.len(), 1);
    assert_eq!(intent.argument_requests.len(), 1);
    assert_eq!(intent.argument_requests[0].0, node_id);
}

#[test]
fn scan_deduplicates_events() {
    let node_id = NodeId::unique();
    let event = EventRef {
        node_id,
        event_idx: 0,
    };

    let intent = super::scan(vec![
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
        intent.events.len(),
        1,
        "duplicate events must collapse to one"
    );
}

#[test]
fn scan_exit_dominates_entire_batch() {
    // Exit is sticky across the whole batch: every other command
    // in the batch is discarded, whether sent before or after.
    let intent = super::scan(vec![
        WorkerMessage::Clear,
        WorkerMessage::ExecuteTerminals,
        WorkerMessage::Exit,
        WorkerMessage::StartEventLoop, // post-Exit: dropped
        WorkerMessage::Update {
            graph: Graph::default(),
            func_lib: Arc::new(FuncLib::default()),
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
        !intent.execute_terminals,
        "pre-Exit execute_terminals must be discarded"
    );
    assert!(intent.events.is_empty());
    assert!(intent.syncs.is_empty());
    assert!(intent.argument_requests.is_empty());
}

#[test]
fn scan_update_overwrites_earlier_update_in_same_batch() {
    // Two Updates in one batch: the last one wins. This is
    // implicit today (Option::replace) but worth pinning since
    // callers do send [Update(A), Update(B)] during rapid edits.
    let empty_graph = Graph::default();
    let func_lib = Arc::new(FuncLib::default());

    let intent = super::scan(vec![
        WorkerMessage::Update {
            graph: empty_graph.clone(),
            func_lib: func_lib.clone(),
        },
        WorkerMessage::Update {
            graph: empty_graph.clone(),
            func_lib: func_lib.clone(),
        },
    ]);

    assert!(matches!(
        intent.graph_state,
        Some(super::GraphOp::Replace(_, _))
    ));
}

// Slot reduction: last-write-wins for graph_state and loop_request.

#[test]
fn scan_clear_then_update_yields_replace() {
    let intent = super::scan(vec![
        WorkerMessage::Clear,
        WorkerMessage::Update {
            graph: Graph::default(),
            func_lib: Arc::new(FuncLib::default()),
        },
    ]);
    assert!(
        matches!(intent.graph_state, Some(super::GraphOp::Replace(_, _))),
        "last write (Update) wins over earlier Clear"
    );
}

#[test]
fn scan_update_then_clear_yields_clear() {
    let intent = super::scan(vec![
        WorkerMessage::Update {
            graph: Graph::default(),
            func_lib: Arc::new(FuncLib::default()),
        },
        WorkerMessage::Clear,
    ]);
    assert!(
        matches!(intent.graph_state, Some(super::GraphOp::Clear)),
        "last write (Clear) wins over earlier Update"
    );
}

#[test]
fn scan_start_then_stop_yields_stop() {
    let intent = super::scan(vec![
        WorkerMessage::StartEventLoop,
        WorkerMessage::StopEventLoop,
    ]);
    assert!(
        matches!(intent.loop_request, Some(super::LoopCommand::Stop)),
        "last write (Stop) wins over earlier Start"
    );
}

#[test]
fn scan_stop_then_start_yields_start() {
    let intent = super::scan(vec![
        WorkerMessage::StopEventLoop,
        WorkerMessage::StartEventLoop,
    ]);
    assert!(
        matches!(intent.loop_request, Some(super::LoopCommand::Start)),
        "last write (Start) wins over earlier Stop"
    );
}

// Integration: end-to-end confirmation that Update-then-Clear in
// one batch leaves the execution graph empty — a subsequent
// InjectEvents hits the empty-graph silent no-op path (no callback).
#[tokio::test]
async fn update_then_clear_in_same_batch_leaves_graph_cleared() {
    let timers_invoker = WorkerEventsFuncLib::default();
    let basic_invoker = BasicFuncLib::default();
    let mut func_lib = basic_invoker.into_func_lib();
    func_lib.merge(timers_invoker.into_func_lib());

    let graph = log_frame_no_graph(&func_lib);
    let frame_event_node_id = graph.by_name("frame event").unwrap().id;

    let (compute_finish_tx, mut compute_finish_rx) = tokio::sync::mpsc::channel(8);
    let mut worker = Worker::new(move |result| {
        compute_finish_tx.try_send(result).ok();
    });

    worker.send_many([
        WorkerMessage::Update {
            graph,
            func_lib: Arc::new(func_lib),
        },
        WorkerMessage::Clear,
        WorkerMessage::InjectEvents {
            events: vec![EventRef {
                node_id: frame_event_node_id,
                event_idx: 0,
            }],
        },
    ]);
    assert_no_callback_within(&mut compute_finish_rx, Duration::from_millis(100)).await;

    worker.exit();
}

// --- `biased` select: commands not starved by events -----------

// Lambda fires as fast as it can; a Stop command must be observed
// and acted on within a bounded time rather than being delayed
// indefinitely by the event stream. Without `biased;` on the
// select!, this test would flake — the fair-random polling could
// sit on the events branch for many iterations.
#[tokio::test]
async fn commands_not_starved_by_fast_event_loop() {
    let output_stream = OutputStream::new();
    let timers_invoker = WorkerEventsFuncLib::default();
    let basic_invoker = BasicFuncLib::with_output_stream(&output_stream).await;
    let mut func_lib = basic_invoker.into_func_lib();
    func_lib.merge(timers_invoker.into_func_lib());

    let graph = log_frame_no_graph(&func_lib);

    let (compute_finish_tx, mut compute_finish_rx) = tokio::sync::mpsc::channel(512);
    let mut worker = Worker::new(move |result| {
        compute_finish_tx.try_send(result).ok();
    });

    worker.send_many([
        WorkerMessage::Update {
            graph,
            func_lib: Arc::new(func_lib),
        },
        WorkerMessage::StartEventLoop,
    ]);

    // Give the loop time to build up momentum.
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(worker.is_event_loop_started());

    // Drain accumulated callbacks so the channel isn't a
    // confounding factor.
    while compute_finish_rx.try_recv().is_ok() {}

    // Send Stop + Sync. Both must be observed within the budget
    // even though lambda events are still being produced.
    let (reply, rx) = oneshot::channel();
    worker.send_many([WorkerMessage::StopEventLoop, WorkerMessage::Sync { reply }]);

    timeout(Duration::from_millis(500), rx)
        .await
        .expect("Sync after StopEventLoop must fire promptly despite event load")
        .expect("Sync sender dropped");

    assert!(
        !worker.is_event_loop_started(),
        "event loop should be stopped"
    );

    worker.exit();
}

// End-to-end: an event fired by a lambda reaches the worker's
// execute path and produces an execution_callback. Covers the
// dedicated bounded-channel flow (no forwarder).
#[tokio::test]
async fn lambda_events_drive_worker_execution() {
    let output_stream = OutputStream::new();
    let timers_invoker = WorkerEventsFuncLib::default();
    let basic_invoker = BasicFuncLib::with_output_stream(&output_stream).await;
    let mut func_lib = basic_invoker.into_func_lib();
    func_lib.merge(timers_invoker.into_func_lib());

    let graph = log_frame_no_graph(&func_lib);

    let (compute_finish_tx, mut compute_finish_rx) = tokio::sync::mpsc::channel(32);
    let mut worker = Worker::new(move |result| {
        compute_finish_tx.try_send(result).ok();
    });

    worker.send_many([
        WorkerMessage::Update {
            graph,
            func_lib: Arc::new(func_lib),
        },
        WorkerMessage::StartEventLoop,
    ]);

    // First callback is the initial execute() inside the
    // start_event_loop path.
    let _ = timeout(Duration::from_millis(500), compute_finish_rx.recv())
        .await
        .expect("initial execute callback")
        .expect("callback channel closed");

    // Subsequent callbacks must come from the lambda firing and
    // the worker draining events from the bounded channel.
    let second = timeout(Duration::from_millis(500), compute_finish_rx.recv())
        .await
        .expect("lambda-driven execute callback")
        .expect("callback channel closed");
    assert!(
        second.is_ok(),
        "lambda-driven execution must succeed: {second:?}"
    );

    worker.exit();
}
