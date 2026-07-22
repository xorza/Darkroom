use std::sync::Arc;

use tokio::sync::Barrier;
use tokio::sync::mpsc::{Receiver, channel};
use tokio::task::JoinHandle;

use crate::execution::event::{EventRef, EventTrigger};
use crate::execution::identity::ExecutionNodeId;
use crate::worker::pause_gate::PauseGate;

pub(crate) const EVENT_LOOP_BACKPRESSURE: usize = 10;

#[derive(Debug)]
pub(crate) struct LambdaPanic {
    pub(crate) node_id: ExecutionNodeId,
    pub(crate) message: String,
}

#[derive(Debug)]
pub(crate) struct ActiveEventLoop {
    join_handles: Vec<(EventRef, JoinHandle<()>)>,
    pub(crate) events: Receiver<EventRef>,
}

impl ActiveEventLoop {
    pub(crate) async fn start(event_triggers: Vec<EventTrigger>, pause_gate: PauseGate) -> Self {
        assert!(!event_triggers.is_empty());

        let (event_tx, events) = channel::<EventRef>(EVENT_LOOP_BACKPRESSURE);
        let participants = event_triggers
            .len()
            .checked_add(1)
            .expect("event trigger count overflow");
        let ready = Arc::new(Barrier::new(participants));
        let mut join_handles = Vec::with_capacity(event_triggers.len());

        for EventTrigger {
            event,
            lambda,
            state,
        } in event_triggers
        {
            let join_handle = tokio::spawn({
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
            join_handles.push((event, join_handle));
        }

        ready.wait().await;
        tokio::task::yield_now().await;

        Self {
            join_handles,
            events,
        }
    }

    pub(crate) async fn stop(&mut self) -> Vec<LambdaPanic> {
        let mut panics = Vec::new();
        for (event, handle) in self.join_handles.drain(..) {
            handle.abort();
            if let Err(error) = handle.await {
                if error.is_panic() {
                    panics.push(LambdaPanic {
                        node_id: event.node_id,
                        message: panic_message(error.into_panic()),
                    });
                } else {
                    assert!(error.is_cancelled(), "event task join error: {error}");
                }
            }
        }
        panics
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

#[derive(Debug, Default)]
pub(crate) struct StopOutcome {
    pub(crate) was_running: bool,
    pub(crate) panics: Vec<LambdaPanic>,
}

pub(crate) async fn stop_event_loop(event_loop: &mut Option<ActiveEventLoop>) -> StopOutcome {
    match event_loop.take() {
        Some(mut active) => {
            let panics = active.stop().await;
            tracing::info!("Event loop stopped");
            StopOutcome {
                was_running: true,
                panics,
            }
        }
        None => StopOutcome::default(),
    }
}
