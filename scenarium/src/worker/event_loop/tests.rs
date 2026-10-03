use super::*;

use tokio::sync::Notify;
use tokio::time;
use tokio::time::{Duration, timeout};

use crate::graph::func::event::EventLambda;
use crate::runtime::shared_any_state::SharedAnyState;
use crate::testing::worker::PATIENCE;

/// A started loop and the node its one trigger fires for.
#[derive(Debug)]
struct SingleLoop {
    active: ActiveEventLoop,
    node_id: NodeId,
}

/// Start an event loop with a single lambda as its only trigger, on a fresh
/// `NodeId` — the shape most `start_event_loop` tests want when they only
/// care about one lambda's behavior.
async fn start_single_event_loop(lambda: EventLambda, pause_gate: PauseGate) -> SingleLoop {
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
    SingleLoop { active, node_id }
}

#[tokio::test]
async fn start_event_loop_forwards_events() {
    let event_lambda = EventLambda::new(|_state| Box::pin(async move {}));
    let SingleLoop {
        mut active,
        node_id,
    } = start_single_event_loop(event_lambda, PauseGate::default()).await;

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

    let SingleLoop {
        mut active,
        node_id,
    } = start_single_event_loop(event_lambda, PauseGate::default()).await;

    // `notify_one` stores a permit, so the lambda proceeds whether or not it has parked yet.
    notify_for_callback.notify_one();

    let event = timeout(PATIENCE, active.recv_event())
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

/// The loop checks the gate after each event, so a closed gate stops the next invocation and
/// reopening it resumes the loop. The lambda parks on `proceed` inside every invocation, so the
/// test steps it one invocation at a time; under paused time, a sleep returns only once every
/// task has run to its next block point, which makes each count exact.
#[tokio::test(start_paused = true)]
async fn pause_gate_blocks_event_loop_iterations() {
    use std::sync::atomic::{AtomicUsize, Ordering};

    let invocations = Arc::new(AtomicUsize::new(0));
    let proceed = Arc::new(Notify::new());
    let event_lambda = EventLambda::new({
        let invocations = Arc::clone(&invocations);
        let proceed = Arc::clone(&proceed);
        move |_state| {
            let invocations = Arc::clone(&invocations);
            let proceed = Arc::clone(&proceed);
            Box::pin(async move {
                invocations.fetch_add(1, Ordering::SeqCst);
                proceed.notified().await;
            })
        }
    });
    let pause_gate = PauseGate::default();
    let SingleLoop {
        mut active,
        node_id,
    } = start_single_event_loop(event_lambda, pause_gate.clone()).await;
    let settle = || time::sleep(Duration::from_millis(1));

    settle().await;
    assert_eq!(
        invocations.load(Ordering::SeqCst),
        1,
        "the first invocation runs"
    );

    // Closed, the gate lets the running invocation finish and send, then holds the next.
    let guard = pause_gate.close();
    proceed.notify_one();
    settle().await;
    assert_eq!(
        active.recv_event().await,
        Some(EventPort {
            node_id,
            event_idx: 0
        })
    );
    assert_eq!(
        invocations.load(Ordering::SeqCst),
        1,
        "a closed gate holds the loop"
    );

    drop(guard);
    settle().await;
    assert_eq!(
        invocations.load(Ordering::SeqCst),
        2,
        "a reopened gate resumes it"
    );

    active.stop().await;
}

#[tokio::test]
async fn lambda_panic_is_captured_not_unwound() {
    let event_lambda = EventLambda::new(|_state| Box::pin(async { panic!("boom in lambda") }));
    let SingleLoop {
        mut active,
        node_id,
    } = start_single_event_loop(event_lambda, PauseGate::default()).await;
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

/// Each event loop start returns a fresh receiver, and stopping one drops its
/// pair, so events it never delivered die with the channel: the old receiver
/// reads closed once its sibling handle is stopped.
#[tokio::test]
async fn stopped_event_loop_channel_is_closed() {
    let event_lambda = EventLambda::new(|_state| Box::pin(async move {}));
    let SingleLoop {
        mut active,
        node_id: _node_id,
    } = start_single_event_loop(event_lambda, PauseGate::default()).await;

    active.stop().await;

    // After stop, all lambda tasks (the sole senders) are aborted →
    // the Receiver must observe channel closure. Drain under a
    // bounded per-recv timeout so a regression that stops closing
    // the channel fails fast instead of wedging the test.
    loop {
        let item = timeout(PATIENCE, active.recv_event())
            .await
            .expect("recv must complete — channel must eventually close after handle.stop()");
        if item.is_none() {
            break;
        }
    }
}
