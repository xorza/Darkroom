use std::sync::Arc;

use tokio::task::{JoinError, JoinHandle};
use tokio_util::sync::CancellationToken;

use common::CancelToken;

use crate::worker::error::WorkerExited;
use crate::worker::mailbox::Mailbox;
use crate::worker::protocol::{WorkerMessage, WorkerReport};
use crate::worker::worker_task::{WorkerLinks, WorkerTask};

pub(crate) mod active_event_loop;
pub(crate) mod activity;
pub(crate) mod batch_intent;
pub(crate) mod error;
mod mailbox;
pub(crate) mod pause_gate;
pub(crate) mod protocol;
pub(crate) mod run_summary;
mod worker_task;

#[derive(Debug)]
pub struct Worker {
    task: Option<JoinHandle<()>>,
    mailbox: Arc<Mailbox>,
    run_cancel: CancelToken,
    shutdown: CancellationToken,
}

impl Worker {
    pub fn new<ExecCallback>(callback: ExecCallback) -> Self
    where
        ExecCallback: Fn(WorkerReport) + Send + Sync + 'static,
    {
        let mailbox = Arc::new(Mailbox::default());
        let run_cancel = CancelToken::new();
        let shutdown = CancellationToken::new();
        let worker_task = WorkerTask::new(WorkerLinks {
            mailbox: Arc::clone(&mailbox),
            callback,
            run_cancel: run_cancel.clone(),
            shutdown: shutdown.clone(),
        });
        let task = tokio::spawn(worker_task.run());

        Self {
            task: Some(task),
            mailbox,
            run_cancel,
            shutdown,
        }
    }

    pub fn request_cancel(&self) {
        self.run_cancel.cancel();
    }

    /// Merge `msg` into the worker's next batch.
    pub fn send(&self, msg: WorkerMessage) -> Result<(), WorkerExited> {
        self.mailbox.post([msg])
    }

    /// Merge `msgs`, in order, into the worker's next batch.
    pub fn send_many<T: IntoIterator<Item = WorkerMessage>>(
        &self,
        msgs: T,
    ) -> Result<(), WorkerExited> {
        self.mailbox.post(msgs)
    }

    /// Cancel active work, drain event tasks, and wait for the worker task to finish.
    pub async fn exit(&mut self) -> Result<(), JoinError> {
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
