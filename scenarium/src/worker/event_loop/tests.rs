use super::*;

use std::sync::Arc;

use tokio::sync::Notify;
use tokio::time::{Duration, timeout};

use crate::execution::report::EventTrigger;
use crate::graph::func::event::EventLambda;
use crate::graph::identity::{EventPort, NodeId};
use crate::runtime::shared_any_state::SharedAnyState;
use crate::worker::pause_gate::PauseGate;

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
            event: EventPort {
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

#[tokio::test]
async fn start_event_loop_forwards_events() {
    let event_lambda = EventLambda::new(|_state| Box::pin(async move {}));
    let (mut active, node_id) = start_single_event_loop(event_lambda, PauseGate::default()).await;

    let event = active
        .recv_event()
        .await
        .expect("Expected event loop event");
    assert_eq!(
        event,
        EventPort {
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

    let event = timeout(Duration::from_millis(200), active.recv_event())
        .await
        .expect("Expected event")
        .expect("Event channel closed");
    assert_eq!(
        event,
        EventPort {
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
    let _ = timeout(Duration::from_millis(100), active.recv_event())
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
    let event_lambda = EventLambda::new(|_state| Box::pin(async { panic!("boom in lambda") }));
    let (mut active, node_id) = start_single_event_loop(event_lambda, PauseGate::default()).await;
    let mut events = Vec::new();

    let wake = active.recv(&mut events).await;
    let EventLoopWake::TaskPanicked(panic) = wake else {
        panic!("panicking lambda must wake the event loop");
    };
    assert!(events.is_empty());
    assert_eq!(panic.node_id, node_id, "panic attributed to its node");
    assert!(
        panic.message.contains("boom in lambda"),
        "panic message preserved: {}",
        panic.message
    );
    assert!(active.stop().await.is_empty());
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
        let item = timeout(Duration::from_millis(500), active.recv_event())
            .await
            .expect("recv must complete — channel must eventually close after handle.stop()");
        if item.is_none() {
            break;
        }
    }
}
