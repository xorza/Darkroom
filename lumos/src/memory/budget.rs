//! [`Budget`]: what one run plans against.

use crate::memory;

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

    /// What a run fanning out on `workers` plans against when it plans for `memory` bytes: the
    /// share [`usable`](crate::memory::usable) leaves of them.
    pub(crate) fn of_memory(memory: u64, workers: usize) -> Self {
        Self::new(memory::usable(memory), workers)
    }

    pub(crate) const fn usable(self) -> u64 {
        self.usable
    }

    pub(crate) const fn workers(self) -> usize {
        self.workers
    }
}
