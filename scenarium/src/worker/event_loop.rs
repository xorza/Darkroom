use std::sync::Arc;

use hashbrown::HashMap;
use tokio::sync::Barrier;
use tokio::sync::mpsc::{Receiver, channel};
use tokio::task::{Id, JoinSet};

use crate::execution::event::EventTrigger;
use crate::execution::identity::ExecutionEventPort;
use crate::execution::identity::ExecutionNodeId;
use crate::worker::pause_gate::PauseGate;

pub(crate) const EVENT_LOOP_BACKPRESSURE: usize = 10;

#[derive(Debug)]
pub(crate) struct LambdaPanic {
    pub(crate) e_node_id: ExecutionNodeId,
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
    task_nodes: HashMap<Id, ExecutionNodeId>,
    pub(crate) events: Receiver<ExecutionEventPort>,
}

impl ActiveEventLoop {
    pub(crate) async fn start(event_triggers: Vec<EventTrigger>, pause_gate: PauseGate) -> Self {
        assert!(!event_triggers.is_empty());

        let (event_tx, events) = channel::<ExecutionEventPort>(EVENT_LOOP_BACKPRESSURE);
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
            let e_node_id = event.e_node_id;
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
            let previous = task_nodes.insert(task.id(), e_node_id);
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

    pub(crate) async fn recv(&mut self, events: &mut Vec<ExecutionEventPort>) -> EventLoopWake {
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
                let e_node_id = self.remove_task(task_id);
                panic!("event task for {e_node_id:?} exited while the event loop was active");
            }
            Err(error) => {
                let e_node_id = self.remove_task(error.id());
                assert!(
                    error.is_panic(),
                    "event task for {e_node_id:?} was cancelled while the event loop was active"
                );
                EventLoopWake::TaskPanicked(LambdaPanic {
                    e_node_id,
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
                    let e_node_id = self.remove_task(task_id);
                    panic!("event task for {e_node_id:?} exited before shutdown");
                }
                Err(error) => {
                    let e_node_id = self.remove_task(error.id());
                    if error.is_panic() {
                        panics.push(LambdaPanic {
                            e_node_id,
                            message: panic_message(error.into_panic()),
                        });
                    } else {
                        assert!(
                            error.is_cancelled(),
                            "event task for {e_node_id:?} failed during shutdown: {error}"
                        );
                    }
                }
            }
        }
        debug_assert!(self.task_nodes.is_empty());
        panics
    }

    fn remove_task(&mut self, task_id: Id) -> ExecutionNodeId {
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
