//! [`Mailbox`]: everything that reaches a worker between two batches, merged as it arrives.

use std::mem;
use std::sync::Mutex;

use common::{CancelToken, MutexExt};
use tokio::sync::Notify;

use crate::exec::program_event::ProgramEvent;
use crate::worker::batch_intent::BatchIntent;
use crate::worker::error::WorkerExited;
use crate::worker::protocol::WorkerMessage;

/// The one input of a worker: the host's messages, the events its loop fires, a cancel and a
/// shutdown, each merged under one lock into the [`BatchIntent`] the worker takes whole, so no
/// rule needs to order one input against another. Bounded by construction: one graph op, node
/// lists bounded by the program, one entry per event. Built as the tokio `Notify` docs build a
/// channel: the one stored permit loses no wake with one consumer. Refused: a bounded channel,
/// whose full queue the host would handle for edits the worker merges anyway; `watch`, whose
/// receiver cannot take and reset the merged state; the fired events on a channel of their own,
/// whose order against a post no rule decides.
#[derive(Debug)]
pub(crate) struct Mailbox {
    pending: Mutex<Pending>,
    wake: Notify,
    /// What the run in flight polls. Cancelled and reset only under `pending`'s lock, so a cancel
    /// lands either before a batch is taken, where it voids that batch's runs, or after, where it
    /// stops that batch's run.
    cancel: CancelToken,
}

#[derive(Debug, Default)]
struct Pending {
    intent: BatchIntent,
    /// The worker is asked to end.
    shutdown: bool,
    /// Set as the worker task ends: a post after it fails.
    closed: bool,
}

/// What [`Mailbox::take_into`] found.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Taken {
    /// A wake whose posts an earlier batch already took.
    Nothing,
    Batch,
    Shutdown,
}

impl Default for Mailbox {
    fn default() -> Self {
        Self {
            pending: Mutex::default(),
            wake: Notify::new(),
            cancel: CancelToken::new(),
        }
    }
}

impl Mailbox {
    /// Merge `msgs` into the pending batch, in order, and wake the worker.
    pub(crate) fn post(
        &self,
        msgs: impl IntoIterator<Item = WorkerMessage>,
    ) -> Result<(), WorkerExited> {
        {
            let mut pending = self.pending.lock_unpoisoned();
            if pending.closed {
                return Err(WorkerExited);
            }
            for msg in msgs {
                pending.intent.push(msg);
            }
        }
        self.wake.notify_one();
        Ok(())
    }

    /// Merge an event the running loop fired, and wake the worker. Dropped once the worker is
    /// gone, as nothing is left to run it.
    pub(crate) fn fire(&self, event: ProgramEvent) {
        {
            let mut pending = self.pending.lock_unpoisoned();
            if pending.closed {
                return;
            }
            pending.intent.push_event(event);
        }
        self.wake.notify_one();
    }

    /// Forget every fired event not taken yet: the loop that fired them is stopped.
    pub(crate) fn void_events(&self) {
        self.pending.lock_unpoisoned().intent.void_events();
    }

    /// Void every run posted so far and cancel the run in flight: what a person who pressed
    /// Cancel meant, as the editor shows no queue of runs to clear apart.
    pub(crate) fn cancel(&self) {
        let mut pending = self.pending.lock_unpoisoned();
        pending.intent.void_runs();
        self.cancel.cancel();
    }

    /// The token a run polls, the same for the worker's life.
    pub(crate) fn cancel_token(&self) -> CancelToken {
        self.cancel.clone()
    }

    /// Ask the worker to end: the run in flight is cancelled, and the next take ends the stream.
    pub(crate) fn shut_down(&self) {
        {
            let mut pending = self.pending.lock_unpoisoned();
            pending.shutdown = true;
            self.cancel.cancel();
        }
        self.wake.notify_one();
    }

    /// Whether the worker is asked to end.
    pub(crate) fn shutting_down(&self) -> bool {
        self.pending.lock_unpoisoned().shutdown
    }

    /// Wait for a post since the last wait. Cancel-safe: a wait dropped after its wake passes the
    /// wake on, as `Notify` forwards it.
    pub(crate) async fn posted(&self) {
        self.wake.notified().await;
    }

    /// Swap the pending batch into `intent`, which is empty, so the next posts merge into the
    /// buffers `intent` held, and clear the cancel in the same critical section: a batch starts
    /// with no cancel a later one was meant for.
    pub(crate) fn take_into(&self, intent: &mut BatchIntent) -> Taken {
        debug_assert!(intent.is_empty(), "the worker applied its last batch whole");
        let mut pending = self.pending.lock_unpoisoned();
        if pending.shutdown {
            return Taken::Shutdown;
        }
        mem::swap(intent, &mut pending.intent);
        self.cancel.reset();
        if intent.is_empty() {
            Taken::Nothing
        } else {
            Taken::Batch
        }
    }

    /// Refuse every later post, as the worker task ends.
    pub(crate) fn close(&self) {
        self.pending.lock_unpoisoned().closed = true;
    }
}

#[cfg(test)]
mod tests;
