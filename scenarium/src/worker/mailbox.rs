//! [`Mailbox`]: what the host sent a worker since its last batch, merged as it arrives.

use std::mem;
use std::sync::Mutex;

use common::MutexExt;
use tokio::sync::Notify;

use crate::worker::batch_intent::BatchIntent;
use crate::worker::error::WorkerExited;
use crate::worker::protocol::WorkerMessage;

/// The messages a worker has not taken yet, each merged as it arrives into the [`BatchIntent`]
/// the worker takes whole: bounded by construction (one graph op, node lists bounded by the
/// program), where a queue of messages grew for as long as a run lasted, each queued `Update`
/// holding its program. Built as the tokio `Notify` docs build a channel: the one stored permit
/// loses no wake with one consumer. Refused: a bounded channel, whose full queue the host would
/// handle for edits the worker merges anyway; `watch`, whose receiver cannot take and reset the
/// merged state.
#[derive(Debug, Default)]
pub(crate) struct Mailbox {
    pending: Mutex<Pending>,
    wake: Notify,
}

#[derive(Debug, Default)]
struct Pending {
    intent: BatchIntent,
    /// Set as the worker task ends: a post after it fails.
    closed: bool,
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

    /// Wait for a post since the last wait. Cancel-safe: a wait dropped after its wake passes the
    /// wake on, as `Notify` forwards it.
    pub(crate) async fn posted(&self) {
        self.wake.notified().await;
    }

    /// Swap the pending batch into `intent`, which is empty, so the next posts merge into the
    /// buffers `intent` held.
    pub(crate) fn take_into(&self, intent: &mut BatchIntent) {
        debug_assert!(intent.is_empty(), "the worker applied its last batch whole");
        mem::swap(intent, &mut self.pending.lock_unpoisoned().intent);
    }

    /// Refuse every later post, as the worker task ends.
    pub(crate) fn close(&self) {
        self.pending.lock_unpoisoned().closed = true;
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use crate::exec::compile::Compiler;
    use crate::graph::Graph;
    use crate::library::Library;
    use crate::worker::batch_intent::{BatchIntent, GraphOp};
    use crate::worker::mailbox::Mailbox;
    use crate::worker::protocol::WorkerMessage;

    /// A replaced `Update` lets its program go as the next one merges, before the worker takes
    /// the batch: the first program's count falls back to the test's own handle, 1, where a
    /// queued message held it, 2, until the worker drained the queue.
    #[test]
    fn a_replaced_update_drops_its_program_on_merge() {
        let program =
            || Arc::new(Compiler::default().compile(&Graph::default(), &Library::default()));
        let (first, second) = (program(), program());
        let mailbox = Mailbox::default();
        mailbox
            .post([WorkerMessage::Update {
                compiled: Arc::clone(&first),
            }])
            .unwrap();
        assert_eq!(Arc::strong_count(&first), 2);
        mailbox
            .post([WorkerMessage::Update {
                compiled: Arc::clone(&second),
            }])
            .unwrap();
        assert_eq!(Arc::strong_count(&first), 1);

        let mut intent = BatchIntent::default();
        mailbox.take_into(&mut intent);
        assert!(
            matches!(&intent.graph_state, Some(GraphOp::Replace(taken)) if Arc::ptr_eq(taken, &second))
        );
    }
}
