//! [`Worker`]: the handle a host drives a background worker task through.

use std::sync::Arc;
use std::time::Duration;

use tokio::time;
use tokio_util::task::AbortOnDropHandle;

use crate::worker::error::{ExitError, WorkerExited};
use crate::worker::mailbox::Mailbox;
use crate::worker::protocol::{WorkerMessage, WorkerReport};
use crate::worker::worker_task::{ReportSink, WorkerTask};

pub(crate) mod active_event_loop;
pub(crate) mod activity;
pub(crate) mod batch_intent;
pub(crate) mod error;
mod mailbox;
pub(crate) mod protocol;
mod run_epoch;
pub(crate) mod run_summary;
mod worker_task;

/// A worker task, and the mailbox every message, cancel and shutdown reaches it through. Dropped,
/// it asks the task to end and aborts it.
#[derive(Debug)]
pub struct Worker {
    /// `None` once [`exit`](Self::exit) has joined it.
    task: Option<AbortOnDropHandle<()>>,
    mailbox: Arc<Mailbox>,
}

impl Worker {
    pub fn new(callback: impl Fn(WorkerReport) + Send + Sync + 'static) -> Self {
        let mailbox = Arc::new(Mailbox::default());
        let task = WorkerTask::new(Arc::clone(&mailbox), ReportSink::new(callback));
        Self {
            task: Some(AbortOnDropHandle::new(tokio::spawn(task.run()))),
            mailbox,
        }
    }

    /// Stop the run in flight and void every run posted before this call. A cancel lands before
    /// the worker takes a batch or after it, so it never misses the run it was meant for nor
    /// reaches a run posted after it.
    pub fn cancel(&self) {
        self.mailbox.cancel();
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

    /// Cancel the run in flight, stop the event loop, and wait for the task to end; past
    /// `patience`, abort it. A cancel is coarse, the running node finishes first, and nothing
    /// stops a lambda that never returns: so the host names how long it waits, as tokio's
    /// `Runtime::shutdown_timeout` does
    /// ([`Runtime`](https://docs.rs/tokio/latest/tokio/runtime/struct.Runtime.html)).
    pub async fn exit(&mut self, patience: Duration) -> Result<(), ExitError> {
        let Some(mut task) = self.task.take() else {
            return Ok(());
        };
        self.mailbox.shut_down();
        match time::timeout(patience, &mut task).await {
            Ok(Ok(())) => Ok(()),
            Ok(Err(error)) => {
                assert!(error.is_panic(), "only `exit` and a drop abort the task");
                Err(ExitError::Panicked(error))
            }
            Err(_elapsed) => {
                task.abort();
                Err(ExitError::TimedOut)
            }
        }
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        // The task's handle, dropped after this, aborts it; the shutdown also cancels a lambda's
        // work off the task, which an abort does not reach.
        self.mailbox.shut_down();
    }
}

#[cfg(test)]
mod tests;
