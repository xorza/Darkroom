//! [`RunEpoch`]: the count of the batches a worker has finished, which paces its event sources.

use tokio::sync::watch;

/// The batches a worker has finished. An event source marks the count seen, fires, and waits for
/// it to move before it fires again, so a source fires at most once per batch however fast its
/// stream yields: the pending batch holds at most one entry per event, and each finished batch
/// lets each source fire once more. `watch`, as only the last count matters.
#[derive(Debug)]
pub(crate) struct RunEpoch(watch::Sender<u64>);

impl Default for RunEpoch {
    fn default() -> Self {
        Self(watch::Sender::new(0))
    }
}

impl RunEpoch {
    /// Count one more finished batch, waking every source waiting on the last.
    pub(crate) fn advance(&self) {
        self.0.send_modify(|count| *count = count.wrapping_add(1));
    }

    /// What a source reads the count through.
    pub(crate) fn subscribe(&self) -> watch::Receiver<u64> {
        self.0.subscribe()
    }
}
