use std::sync::Arc;

use hashbrown::HashMap;
use tokio::sync::Barrier;
use tokio::sync::mpsc::{Receiver, channel};
use tokio::task::{Id, JoinSet};

use crate::execution::report::EventTrigger;
use crate::graph::identity::EventPort;
use crate::graph::identity::NodeId;
use crate::worker::pause_gate::PauseGate;

pub(crate) const EVENT_LOOP_BACKPRESSURE: usize = 10;

#[derive(Debug)]
pub(crate) struct LambdaPanic {
    pub(crate) node_id: NodeId,
    pub(crate) message: String,
}

#[derive(Debug)]
pub(crate) enum EventLoopWake {
    Events,
    TaskPanicked(LambdaPanic),
}

#[derive(Debug)]
pub(crate) struct ActiveEventLoop {
    tasks: JoinSet<()>,
    task_nodes: HashMap<Id, NodeId>,
    events: Receiver<EventPort>,
}

impl ActiveEventLoop {
    pub(crate) async fn start(event_triggers: Vec<EventTrigger>, pause_gate: PauseGate) -> Self {
        assert!(!event_triggers.is_empty());

        let (event_tx, events) = channel::<EventPort>(EVENT_LOOP_BACKPRESSURE);
        let participants = event_triggers
            .len()
            .checked_add(1)
            .expect("event trigger count overflow");
        let ready = Arc::new(Barrier::new(participants));
        let mut tasks = JoinSet::new();
        let mut task_nodes = HashMap::with_capacity(event_triggers.len());

        for EventTrigger {
            event,
            lambda,
            state,
        } in event_triggers
        {
            let node_id = event.node_id;
            let task = tasks.spawn({
                let event_tx = event_tx.clone();
                let ready = ready.clone();
                let pause_gate = pause_gate.clone();

                async move {
                    ready.wait().await;
                    loop {
                        lambda.invoke(state.clone()).await;
                        if event_tx.send(event).await.is_err() {
                            return;
                        }
                        pause_gate.wait().await;
                        tokio::task::yield_now().await;
                    }
                }
            });
            let previous = task_nodes.insert(task.id(), node_id);
            debug_assert!(previous.is_none(), "duplicate event task id");
        }

        ready.wait().await;
        tokio::task::yield_now().await;

        Self {
            tasks,
            task_nodes,
            events,
        }
    }

    pub(crate) async fn recv(&mut self, events: &mut Vec<EventPort>) -> EventLoopWake {
        // Observe failures even when another task keeps the event stream continuously ready.
        let task_result = tokio::select! {
            biased;
            result = self.tasks.join_next_with_id() => result,
            count = self.events.recv_many(events, EVENT_LOOP_BACKPRESSURE) => {
                if count > 0 {
                    return EventLoopWake::Events;
                }
                self.tasks.join_next_with_id().await
            }
        }
        .expect("active event loop must contain a task");

        match task_result {
            Ok((task_id, ())) => {
                let node_id = self.remove_task(task_id);
                panic!("event task for {node_id:?} exited while the event loop was active");
            }
            Err(error) => {
                let node_id = self.remove_task(error.id());
                assert!(
                    error.is_panic(),
                    "event task for {node_id:?} was cancelled while the event loop was active"
                );
                EventLoopWake::TaskPanicked(LambdaPanic {
                    node_id,
                    message: panic_message(error.into_panic()),
                })
            }
        }
    }

    pub(crate) async fn stop(&mut self) -> Vec<LambdaPanic> {
        self.tasks.abort_all();
        let mut panics = Vec::new();
        while let Some(result) = self.tasks.join_next_with_id().await {
            match result {
                Ok((task_id, ())) => {
                    let node_id = self.remove_task(task_id);
                    panic!("event task for {node_id:?} exited before shutdown");
                }
                Err(error) => {
                    let node_id = self.remove_task(error.id());
                    if error.is_panic() {
                        panics.push(LambdaPanic {
                            node_id,
                            message: panic_message(error.into_panic()),
                        });
                    } else {
                        assert!(
                            error.is_cancelled(),
                            "event task for {node_id:?} failed during shutdown: {error}"
                        );
                    }
                }
            }
        }
        debug_assert!(self.task_nodes.is_empty());
        panics
    }

    fn remove_task(&mut self, task_id: Id) -> NodeId {
        self.task_nodes
            .remove(&task_id)
            .unwrap_or_else(|| panic!("event task {task_id} has no node attribution"))
    }
}

fn panic_message(payload: Box<dyn std::any::Any + Send>) -> String {
    if let Some(message) = payload.downcast_ref::<&str>() {
        (*message).to_string()
    } else if let Some(message) = payload.downcast_ref::<String>() {
        message.clone()
    } else {
        "unknown panic".to_string()
    }
}

#[cfg(test)]
pub(crate) mod internals {
    use crate::graph::identity::EventPort;
    use crate::worker::event_loop::ActiveEventLoop;

    impl ActiveEventLoop {
        /// Raw next event from the loop's channel, bypassing the join-set
        /// machinery `recv` also watches — what tests want when they only
        /// care about the event stream itself.
        pub(crate) async fn recv_event(&mut self) -> Option<EventPort> {
            self.events.recv().await
        }
    }
}

#[cfg(test)]
mod tests {
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
        let (mut active, node_id) =
            start_single_event_loop(event_lambda, PauseGate::default()).await;

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

        let (mut active, node_id) =
            start_single_event_loop(event_lambda, PauseGate::default()).await;

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

        let (mut active, _node_id) =
            start_single_event_loop(event_lambda, pause_gate.clone()).await;

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
        let (mut active, node_id) =
            start_single_event_loop(event_lambda, PauseGate::default()).await;
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
        let (mut active, _node_id) =
            start_single_event_loop(event_lambda, PauseGate::default()).await;

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
}
