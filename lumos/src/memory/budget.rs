//! [`Budget`]: what one run plans against.

/// What one run plans against, read once at its entry: the bytes it may hold — the share
/// [`usable`](crate::memory::usable) leaves of the memory it plans for — and the
/// workers its stages fan out on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Budget {
    usable: u64,
    workers: usize,
}

impl Budget {
    pub(crate) const fn new(usable: u64, workers: usize) -> Self {
        debug_assert!(workers > 0, "a run has one worker at least");
        Self { usable, workers }
    }

    pub(crate) const fn usable(self) -> u64 {
        self.usable
    }

    pub(crate) const fn workers(self) -> usize {
        self.workers
    }
}
