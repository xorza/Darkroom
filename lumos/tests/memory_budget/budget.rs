//! [`Budget`]: the memory a run plans against, and the heap it may hold for it.

use lumos::RunReport;

/// The share of `memory_override` a run plans to commit: `memory::MEMORY_PERCENT`.
const BUDGET_PERCENT: u64 = 75;

/// The heap no plan charges: the combine's reduction scratch on each worker, the frame list and
/// the result vectors, and each worker's own bookkeeping. None grows with a frame's pixels; at the
/// peak of a spilled registered run they held 31 KB.
const UNPLANNED_BYTES: u64 = 64 << 10;

/// A run's `memory_override`: the memory it plans as if the machine had.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Budget {
    memory_override: u64,
}

impl Budget {
    pub(crate) const fn kib(kib: u64) -> Self {
        Self {
            memory_override: kib << 10,
        }
    }

    pub(crate) const fn mib(mib: u64) -> Self {
        Self::kib(mib << 10)
    }

    pub(crate) const fn memory_override(self) -> u64 {
        self.memory_override
    }

    /// The heap a run plans to hold at most.
    pub(crate) const fn bytes(self) -> u64 {
        self.memory_override * BUDGET_PERCENT / 100
    }

    /// Assert a run of `label` held at most `peak_bytes`: the budget, what its `report` says it
    /// held past it when not even a stage's smallest step fit, and [`UNPLANNED_BYTES`].
    pub(crate) fn assert_held(self, label: &str, peak_bytes: usize, report: &RunReport) {
        let budget = self.bytes();
        let overcommit = report.overcommit_bytes;
        assert!(
            peak_bytes as u64 <= budget + overcommit + UNPLANNED_BYTES,
            "{label}: the run held {peak_bytes} B at its peak, over its budget of {budget} B, \
             {overcommit} B of overcommit and {UNPLANNED_BYTES} B unplanned"
        );
    }
}
