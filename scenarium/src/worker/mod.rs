use tokio::sync::mpsc::{UnboundedSender, unbounded_channel};
use tokio::task::JoinHandle;

use common::CancelToken;

use crate::worker::protocol::{WorkerExited, WorkerMessage, WorkerReport};
use crate::worker::task::WorkerTask;

pub(crate) mod batch;
pub(crate) mod event_loop;
pub(crate) mod pause_gate;
pub(crate) mod protocol;
pub(crate) mod status;
mod task;

#[derive(Debug)]
pub struct Worker {
    thread_handle: Option<JoinHandle<()>>,
    tx: UnboundedSender<WorkerMessage>,
    cancel: CancelToken,
}

impl Worker {
    pub fn new<ExecutionCallback>(callback: ExecutionCallback) -> Self
    where
        ExecutionCallback: Fn(WorkerReport) + Send + Sync + 'static,
    {
        let (tx, rx) = unbounded_channel::<WorkerMessage>();
        let cancel = CancelToken::new();
        let task = WorkerTask::new(rx, callback, cancel.clone());
        let thread_handle = tokio::spawn(task.run());

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
        self.tx.send(msg).map_err(|_| WorkerExited)
    }

    pub fn send_many<T: IntoIterator<Item = WorkerMessage>>(
        &self,
        msgs: T,
    ) -> std::result::Result<(), WorkerExited> {
        for msg in msgs {
            self.send(msg)?;
        }
        Ok(())
    }

    pub fn exit(&mut self) {
        self.tx.send(WorkerMessage::Exit).ok();
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
