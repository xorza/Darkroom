use tokio::sync::mpsc::{UnboundedSender, unbounded_channel};
use tokio::task::JoinHandle;

use common::CancelToken;

use crate::worker::protocol::{WorkerExited, WorkerMessage, WorkerReport};

pub(crate) mod batch;
pub(crate) mod event_loop;
pub(crate) mod pause_gate;
pub(crate) mod protocol;
pub(crate) mod status;
mod task;

#[derive(Debug)]
pub struct Worker {
    thread_handle: Option<JoinHandle<()>>,
    tx: UnboundedSender<Vec<WorkerMessage>>,
    cancel: CancelToken,
}

impl Worker {
    pub fn new<ExecutionCallback>(callback: ExecutionCallback) -> Self
    where
        ExecutionCallback: Fn(WorkerReport) + Send + Sync + 'static,
    {
        let (tx, rx) = unbounded_channel::<Vec<WorkerMessage>>();
        let cancel = CancelToken::new();
        let thread_handle = tokio::spawn({
            let cancel = cancel.clone();
            async move {
                task::run(rx, callback, cancel).await;
            }
        });

        Self {
            thread_handle: Some(thread_handle),
            tx,
            cancel,
        }
    }

    pub fn request_cancel(&self) {
        self.cancel.cancel();
    }

    pub fn send(&self, msg: WorkerMessage) -> std::result::Result<(), WorkerExited> {
        self.send_many([msg])
    }

    pub fn send_many<T: IntoIterator<Item = WorkerMessage>>(
        &self,
        msgs: T,
    ) -> std::result::Result<(), WorkerExited> {
        let msgs = msgs.into_iter().collect::<Vec<_>>();
        if msgs.is_empty() {
            return Ok(());
        }
        self.tx.send(msgs).map_err(|_| WorkerExited)
    }

    pub fn exit(&mut self) {
        self.tx.send(vec![WorkerMessage::Exit]).ok();
        self.request_cancel();

        if let Some(thread_handle) = self.thread_handle.take() {
            thread_handle.abort();
        }
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        self.exit();
    }
}

#[cfg(test)]
mod tests;
