//! [`Tier`]: where a run keeps its frames, decided once from its memory plan.

use std::path::Path;

use imaginarium::Buffer2;

use crate::frame::admitted::Admitted;
use crate::frame::frame_quality::FrameQuality;
use crate::frame_store::error::FrameStoreError;
use crate::frame_store::run_scratch::RunScratch;
use crate::frame_store::stored_frame::{SpillFrame, StoredFrame};
use crate::image::stackable_image::StackableImage;
use crate::memory::budget::Budget;
use crate::memory::run_plan::RunPlan;

/// Where a whole run keeps its frames: every one resident, or every one through the frame store's
/// memory maps. Chosen once from the run's [`RunPlan`], and the one value every stage that stores a
/// frame or sizes a combine chunk reads, so no two of them can disagree.
#[derive(Debug)]
pub(crate) enum Tier {
    /// Every plane in RAM: the combine walks whole planes and needs no budget.
    Resident,
    /// Planes in `scratch`, memory-mapped back and read in row chunks `combine` budgets.
    Spilled {
        scratch: RunScratch,
        combine: CombineBudget,
    },
}

/// What a spilled combine plans its rows against: the run's budget, the bytes the run holds beside
/// the combine for its whole pass, and the most the stages before it held past the budget, which
/// the run reports with the combine's own.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct CombineBudget {
    pub(crate) budget: Budget,
    pub(crate) beside_bytes: usize,
    pub(crate) prior_overcommit: u64,
}

impl CombineBudget {
    /// `budget` alone, nothing beside the combine and nothing past the budget before it.
    pub(crate) const fn new(budget: Budget) -> Self {
        Self {
            budget,
            beside_bytes: 0,
            prior_overcommit: 0,
        }
    }
}

impl Tier {
    /// Resident when `plan` keeps the run in memory; spilled to a scratch under `cache_dir`
    /// otherwise, its combine budgeted against the plan's budget. The disk is checked by
    /// [`RunStorage::tier`](crate::frame_store::run_storage::RunStorage::tier), which knows
    /// everything else the run writes there.
    ///
    /// # Errors
    ///
    /// The scratch cannot be made.
    pub(crate) fn for_plan(plan: &RunPlan, cache_dir: &Path) -> Result<Self, FrameStoreError> {
        if plan.resident {
            return Ok(Self::Resident);
        }
        let prior_overcommit = plan.prior_overcommit();
        if prior_overcommit > 0 {
            tracing::warn!(
                overcommit_bytes = prior_overcommit,
                "the smallest step of a stage does not fit the memory budget"
            );
        }
        Ok(Self::Spilled {
            scratch: RunScratch::create(cache_dir)?,
            combine: CombineBudget {
                prior_overcommit,
                ..CombineBudget::new(plan.budget)
            },
        })
    }

    /// The scratch a spilled tier writes to.
    pub(crate) const fn scratch(&self) -> Option<&RunScratch> {
        match self {
            Self::Resident => None,
            Self::Spilled { scratch, .. } => Some(scratch),
        }
    }

    pub(crate) const fn spills(&self) -> bool {
        matches!(self, Self::Spilled { .. })
    }

    /// This tier, its combine leaving room for `bytes` the run holds beside it for its whole pass.
    pub(crate) const fn holding_beside(mut self, bytes: usize) -> Self {
        if let Self::Spilled { combine, .. } = &mut self {
            combine.beside_bytes = bytes;
        }
        self
    }

    /// What a spilled combine plans its rows against; `None` for a resident one, which reads its
    /// frames where they lie.
    pub(crate) const fn combine_budget(&self) -> Option<CombineBudget> {
        match self {
            Self::Resident => None,
            Self::Spilled { combine, .. } => Some(*combine),
        }
    }

    /// Keep one frame: moved in whole on the resident tier, written to the scratch and mapped
    /// back on the spilled one.
    pub(crate) fn store(
        &self,
        image: impl StackableImage,
        quality: FrameQuality<Buffer2<f32>>,
        admitted: Admitted,
    ) -> Result<StoredFrame, FrameStoreError> {
        match self {
            Self::Resident => Ok(StoredFrame::from_memory(image, quality, admitted)),
            Self::Spilled { scratch, .. } => SpillFrame {
                image: &image,
                quality: &quality,
                admitted,
            }
            .spill(scratch),
        }
    }
}
