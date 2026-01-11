use hashbrown::HashMap;
use serde::{Deserialize, Serialize};
use std::future::Future;
use std::sync::Arc;

use tokio::task::JoinHandle;
use tracing::info;

use crate::graph::NodeId;

#[derive(Debug)]
struct NodeEvent {
    node_id: NodeId,
    trigger: Arc<tokio::sync::Mutex<Option<tokio::sync::mpsc::Sender<EventId>>>>,
    join_handles: Vec<JoinHandle<()>>,
}

#[derive(Debug)]
pub struct NodeEventManager {
    frame_tx: tokio::sync::broadcast::Sender<()>,
    event_rx: tokio::sync::mpsc::Sender<EventId>,
    node_events: HashMap<NodeId, NodeEvent>,
}

#[derive(Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct EventId {
    pub node_id: NodeId,
    pub event_idx: usize,
}

#[derive(Debug)]
pub struct EventTrigger {
    trigger: Arc<tokio::sync::Mutex<Option<tokio::sync::mpsc::Sender<EventId>>>>,
    node_id: NodeId,
}

impl NodeEventManager {
    pub fn new(
        frame_tx: tokio::sync::broadcast::Sender<()>,
        event_rx: tokio::sync::mpsc::Sender<EventId>,
    ) -> Self {
        Self {
            frame_tx,
            event_rx,
            node_events: HashMap::new(),
        }
    }

    pub fn stop_node_events(&mut self, node_id: NodeId) {
        let mut node_event = self
            .node_events
            .remove(&node_id)
            .expect("Node event not found");

        Self::stop(&mut node_event);
    }

    fn stop(node_event: &mut NodeEvent) {
        {
            // revoke old trigger
            let mut trigger = node_event
                .trigger
                .try_lock()
                .expect("Event trigger mutex is already locked");
            *trigger = None;
        }

        node_event.join_handles.iter().for_each(JoinHandle::abort);
        node_event.join_handles.clear();
    }

    pub fn start_node_event_loop<F, Fut>(
        &mut self,
        runtime: &tokio::runtime::Handle,
        event_id: EventId,
        new_future: F,
    ) where
        F: Fn() -> Fut + Send + Copy + 'static,
        Fut: Future<Output = ()> + Send,
    {
        let node_event = self
            .node_events
            .entry(event_id.node_id)
            .or_insert_with(|| NodeEvent {
                node_id: event_id.node_id,
                trigger: Arc::new(tokio::sync::Mutex::new(Some(self.event_rx.clone()))),
                join_handles: Vec::new(),
            });

        let trigger = node_event.trigger.clone();
        let node_id = event_id.node_id;
        let event_index = event_id.event_idx;
        let mut frame_rx = self.frame_tx.subscribe();

        let join_handle = runtime.spawn(async move {
            loop {
                let future = (new_future)();
                future.await;

                let result = {
                    let trigger = trigger.lock().await;
                    if trigger.is_none() {
                        info!("Event loop stopped for event {:?}", event_index);
                        break;
                    }

                    trigger
                        .as_ref()
                        .expect("Event trigger missing")
                        .send(EventId {
                            node_id,
                            event_idx: event_index,
                        })
                        .await
                };
                if result.is_err() {
                    info!("Failed to send event {:?}", event_index);
                    break;
                }

                let result = frame_rx.recv().await;
                if result.is_err() {
                    info!("Failed to receive frame {:?}", event_index);
                    break;
                }
            }
        });
        node_event.join_handles.push(join_handle);
    }
}

impl Drop for NodeEventManager {
    fn drop(&mut self) {
        self.node_events.values_mut().for_each(Self::stop);
        self.node_events.clear();
    }
}

#[cfg(test)]
mod tests {
    use crate::event::{EventId, NodeEventManager};
    use crate::graph::NodeId;

    #[tokio::test]
    async fn test_event() {
        let (event_tx, mut event_rx) = tokio::sync::mpsc::channel::<EventId>(5);
        let (frame_tx, _frame_rx) = tokio::sync::broadcast::channel::<()>(5);

        let mut event_owner = NodeEventManager::new(frame_tx.clone(), event_tx);
        let node_id = NodeId::unique();

        let runtime = tokio::runtime::Handle::current();

        event_owner.start_node_event_loop(
            &runtime,
            EventId {
                node_id,
                event_idx: 0,
            },
            || async move {
                tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
            },
        );
        event_owner.start_node_event_loop(
            &runtime,
            EventId {
                node_id,
                event_idx: 1,
            },
            || async move {
                tokio::time::sleep(tokio::time::Duration::from_millis(300)).await;
            },
        );

        async fn next_event(event_rx: &mut tokio::sync::mpsc::Receiver<EventId>) -> EventId {
            tokio::time::timeout(tokio::time::Duration::from_millis(500), event_rx.recv())
                .await
                .expect("Timed out waiting for event")
                .expect("Missing event")
        }

        let event = next_event(&mut event_rx).await;
        assert_eq!(event.event_idx, 0);

        let event = next_event(&mut event_rx).await;
        assert_eq!(event.event_idx, 1);

        frame_tx.send(()).expect("Failed to send frame signal");

        let event = next_event(&mut event_rx).await;
        assert_eq!(event.event_idx, 0);

        event_owner.stop_node_events(node_id);

        event_owner.start_node_event_loop(
            &runtime,
            EventId {
                node_id,
                event_idx: 2,
            },
            || async move {
                tokio::time::sleep(tokio::time::Duration::from_millis(18)).await;
            },
        );

        let event = next_event(&mut event_rx).await;
        assert_eq!(event.event_idx, 2);
    }
}
