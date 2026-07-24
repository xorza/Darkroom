use tokio::sync::mpsc::{UnboundedSender, unbounded_channel};
use tokio::task::{JoinError, JoinHandle};
use tokio_util::sync::CancellationToken;

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
    task: Option<JoinHandle<()>>,
    tx: UnboundedSender<WorkerMessage>,
    run_cancel: CancelToken,
    shutdown: CancellationToken,
}

impl Worker {
    pub fn new<ExecutionCallback>(callback: ExecutionCallback) -> Self
    where
        ExecutionCallback: Fn(WorkerReport) + Send + Sync + 'static,
    {
        let (tx, rx) = unbounded_channel::<WorkerMessage>();
        let run_cancel = CancelToken::new();
        let shutdown = CancellationToken::new();
        let worker_task = WorkerTask::new(rx, callback, run_cancel.clone(), shutdown.clone());
        let task = tokio::spawn(worker_task.run());

        Self {
            task: Some(task),
            tx,
            run_cancel,
            shutdown,
        }
    }

    pub fn request_cancel(&self) {
        self.run_cancel.cancel();
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

    /// Cancel active work, drain event tasks, and wait for the worker task to finish.
    pub async fn exit(&mut self) -> std::result::Result<(), JoinError> {
        self.request_exit();
        let result = match self.task.as_mut() {
            Some(task) => task.await,
            None => return Ok(()),
        };
        self.task.take();
        result
    }

    fn request_exit(&self) {
        self.shutdown.cancel();
        self.run_cancel.cancel();
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        self.request_exit();
        // Drop cannot await; callers needing structured cleanup use `exit`.
        if let Some(task) = self.task.take() {
            task.abort();
        }
    }
}

#[cfg(test)]
mod tests;
