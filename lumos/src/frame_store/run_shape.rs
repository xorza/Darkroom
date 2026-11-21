//! [`RunShape`]: what a run holds and writes, and the tier its plan keeps its frames in.

use crate::frame_store::error::FrameStoreError;
use crate::frame_store::run_storage::{RunStorage, RunWrites};
use crate::frame_store::storage_tier::StorageTier;
use crate::memory::budget::Budget;
use crate::memory::run_plan::RunPlan;
use crate::memory::stage::Stage;

/// What one run of `frame_count` frames holds and writes: its stages, the bytes it writes to its
/// disk, and the bytes it holds beside its combine for the combine's whole pass. Every entry
/// describes its run as one, and [`Self::tier`] is the one place a plan becomes a tier.
#[derive(Debug, Clone, Copy)]
pub(crate) struct RunShape<const N: usize> {
    pub(crate) frame_count: usize,
    pub(crate) stages: [Stage; N],
    pub(crate) writes: RunWrites,
    pub(crate) beside_bytes: usize,
}

/// The tier a run keeps its frames in, and the plan that chose it, whose widths its stages run at.
#[derive(Debug)]
pub(crate) struct PlannedTier {
    pub(crate) tier: StorageTier,
    pub(crate) plan: RunPlan,
}

impl<const N: usize> RunShape<N> {
    /// The run planned under `budget`, its frames kept on `storage` once its disk is known to hold
    /// what the run writes there, its combine leaving room for what the run holds beside it.
    ///
    /// # Errors
    ///
    /// The scratch cannot be made, or [`FrameStoreError::InsufficientSpace`].
    pub(crate) fn tier(
        &self,
        budget: Budget,
        storage: RunStorage<'_>,
    ) -> Result<PlannedTier, FrameStoreError> {
        let plan = RunPlan::of(self.frame_count, &self.stages, budget);
        let tier = storage
            .tier(&plan, self.writes)?
            .holding_beside(self.beside_bytes);
        Ok(PlannedTier { tier, plan })
    }
}
