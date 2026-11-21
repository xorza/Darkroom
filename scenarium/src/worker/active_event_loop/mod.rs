//! [`ActiveEventLoop`]: one task per armed event, each firing into the worker's mailbox.

use std::convert::Infallible;
use std::future;
use std::panic;
use std::sync::Arc;

use tokio::task::JoinSet;

use crate::exec::report::armed_event::ArmedEvent;
use crate::worker::mailbox::Mailbox;
use crate::worker::run_epoch::RunEpoch;

/// The running loop's tasks. A task never ends of itself: it is stopped, or it panics, and a
/// panic is resumed on the worker task, as a func lambda's ends it, so a lambda's panic is one
/// behavior whichever lambda it is and whatever the build. Refused: catching an event lambda's
/// panic and reporting it against its node, which only an unwind build could, and which keeps a
/// state the panic left half-changed in use.
#[derive(Debug)]
pub(crate) struct ActiveEventLoop {
    tasks: JoinSet<Infallible>,
}

impl ActiveEventLoop {
    pub(crate) fn start(
        armed_events: Vec<ArmedEvent>,
        mailbox: &Arc<Mailbox>,
        epoch: &RunEpoch,
    ) -> Self {
        assert!(!armed_events.is_empty());

        let mut tasks = JoinSet::new();
        for ArmedEvent {
            event,
            lambda,
            state,
        } in armed_events
        {
            let mailbox = Arc::clone(mailbox);
            let mut epoch = epoch.subscribe();
            let mut fires = lambda.fires(state);
            tasks.spawn(async move {
                loop {
                    fires.next().await;
                    // Seen before the fire, so a batch that ends between the fire and the wait
                    // still counts as the one this source waits for.
                    epoch.borrow_and_update();
                    mailbox.fire(event);
                    if epoch.changed().await.is_err() {
                        // The worker is gone, and its abort of this task follows.
                        return future::pending().await;
                    }
                }
            });
        }
        Self { tasks }
    }

    /// Resolves only with a task's panic, which it resumes on the caller.
    pub(crate) async fn failed(&mut self) -> Infallible {
        let joined = self
            .tasks
            .join_next()
            .await
            .expect("an active loop holds its tasks");
        match joined {
            Ok(never) => never,
            Err(error) => panic::resume_unwind(error.into_panic()),
        }
    }

    /// Abort every task and wait until none runs, resuming the first that had panicked.
    pub(crate) async fn stop(&mut self) {
        self.tasks.abort_all();
        while let Some(joined) = self.tasks.join_next().await {
            match joined {
                Ok(never) => match never {},
                Err(error) if error.is_panic() => panic::resume_unwind(error.into_panic()),
                Err(error) => assert!(
                    error.is_cancelled(),
                    "an event task ends cancelled: {error}"
                ),
            }
        }
    }
}

#[cfg(test)]
mod tests;
