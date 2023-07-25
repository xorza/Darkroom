use std::future::Future;
use std::sync::Arc;

use tokio::runtime::Runtime;
use tokio::sync::mpsc::Sender;
use tokio::sync::Mutex;
use tokio::task::JoinHandle;

use crate::graph::NodeId;

#[derive(Debug)]
pub struct EventOwner {
    trigger: Arc<Mutex<Option<Sender<EventId>>>>,
    node_id: NodeId,
    rx: Sender<EventId>,
    join_handles: Vec<JoinHandle<()>>,
}

#[derive(Debug)]
pub struct EventId {
    node_id: NodeId,
    event_index: u32,
}
#[derive(Debug)]
pub struct EventTrigger {
    trigger: Arc<Mutex<Option<Sender<EventId>>>>,
    node_id: NodeId,
}


impl EventOwner {
    pub fn new(node_id: NodeId, rx: Sender<EventId>) -> Self {
        Self {
            trigger: Arc::new(Mutex::new(Some(rx.clone()))),
            node_id,
            rx,
            join_handles: Vec::new(),
        }
    }

    pub fn stop_all_event_loops(&mut self) {
        {
            // revoke old trigger
            let mut trigger = self.trigger.blocking_lock();
            *trigger = None;
        }
        self.join_handles
            .iter()
            .for_each(JoinHandle::abort);
        self.join_handles.clear();
        self.trigger = Arc::new(Mutex::new(Some(self.rx.clone())));
    }

    pub fn start_event_loop<F, Fut>(
        &mut self,
        runtime: &Runtime,
        event_index: u32,
        new_future: F,
    )
    where F: Fn() -> Fut + Send + Copy + 'static,
          Fut: Future<Output=()> + Send,
    {
        let trigger = self.trigger.clone();
        let node_id = self.node_id;

        let join_handle = runtime.spawn(async move {
            loop {
                let future = (new_future)();
                future.await;

                let trigger = trigger
                    .lock()
                    .await;
                if trigger.is_none() {
                    println!("Event loop stopped for event {:?}", event_index);
                    break;
                }
                let result = trigger
                    .as_ref()
                    .unwrap()
                    .send(EventId {
                        node_id,
                        event_index,
                    })
                    .await;
                if result.is_err() {
                    println!("Failed to send event {:?}", event_index);
                    break;
                }
            }
        });
        self.join_handles.push(join_handle);
    }
}

#[cfg(test)]
mod tests {
    use tokio::sync::mpsc::channel;

    use crate::event::EventOwner;
    use crate::graph::NodeId;

    #[test]
    fn test_event() {
        let (tx, mut rx) = channel(5);
        let mut event_owner = EventOwner::new(NodeId::unique(), tx);
        let runtime = tokio::runtime::Runtime::new().unwrap();

        event_owner.start_event_loop(
            &runtime,
            0,
            || async move {
                tokio::time::sleep(tokio::time::Duration::from_millis(4)).await;
            },
        );
        event_owner.start_event_loop(
            &runtime,
            1,
            || async move {
                tokio::time::sleep(tokio::time::Duration::from_millis(6)).await;
            },
        );


        let event = rx
            .blocking_recv()
            .unwrap();
        assert_eq!(event.event_index, 0);

        let event = rx
            .blocking_recv()
            .unwrap();
        assert_eq!(event.event_index, 1);

        let event = rx
            .blocking_recv()
            .unwrap();
        assert_eq!(event.event_index, 0);

        event_owner.stop_all_event_loops();

        event_owner.start_event_loop(
            &runtime,
            2,
            || async move {
                tokio::time::sleep(tokio::time::Duration::from_millis(8)).await;
            },
        );

        let event = rx
            .blocking_recv()
            .unwrap();
        assert_eq!(event.event_index, 2);
    }
}