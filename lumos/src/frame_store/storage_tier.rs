//! [`StorageTier`]: where a run keeps its frames, decided once from its memory plan.

use std::path::Path;

use crate::frame::admitted::Admitted;
use crate::frame_store::error::FrameStoreError;
use crate::frame_store::frame_buffers::{FrameBuffers, NewFrame};
use crate::frame_store::run_scratch::RunScratch;
use crate::frame_store::stored_frame::{FrameWrite, SetKind, StoredFrame};
use crate::memory::budget::Budget;
use crate::memory::run_plan::RunPlan;

/// Where a whole run keeps its frames: every one resident, or every one through the frame store's
/// memory maps. Chosen once from the run's [`RunPlan`], and the one value every stage that stores a
/// frame or sizes a combine chunk reads, so no two of them can disagree.
#[derive(Debug)]
pub(crate) enum StorageTier {
    /// Every plane in RAM: the combine walks whole planes and needs no budget. `planned_peak_bytes`
    /// is the peak the run's plan expects; `None` for frames handed over with no plan.
    Resident { planned_peak_bytes: Option<u64> },
    /// Planes in `scratch`, memory-mapped back and read in row chunks `combine` budgets.
    Spilled {
        scratch: RunScratch,
        combine: CombineBudget,
    },
}

/// What a spilled combine plans its rows against: the run's budget, the bytes the run holds beside
/// the combine for its whole pass, and the most the stages before it held past the budget, which
/// the run reports with the combine's own, as it reports the peak its plan expects.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct CombineBudget {
    pub(crate) budget: Budget,
    pub(crate) beside_bytes: usize,
    pub(crate) prior_overcommit: u64,
    pub(crate) planned_peak_bytes: Option<u64>,
}

/// A frame a tier stored, and the buffers it came in when the tier wrote them rather than moved
/// them in, for their producer to fill again.
#[derive(Debug)]
pub(crate) struct Stored<B> {
    pub(crate) frame: StoredFrame,
    pub(crate) returned: Option<B>,
}

impl CombineBudget {
    /// `budget` alone, nothing beside the combine, nothing past the budget before it, and no plan.
    pub(crate) const fn new(budget: Budget) -> Self {
        Self {
            budget,
            beside_bytes: 0,
            prior_overcommit: 0,
            planned_peak_bytes: None,
        }
    }
}

impl StorageTier {
    /// Every plane in RAM, for frames handed over with no plan.
    pub(crate) const UNPLANNED: Self = Self::Resident {
        planned_peak_bytes: None,
    };

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
            return Ok(Self::Resident {
                planned_peak_bytes: Some(plan.peak_bytes),
            });
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
                planned_peak_bytes: Some(plan.peak_bytes),
                ..CombineBudget::new(plan.budget)
            },
        })
    }

    /// The peak the run's plan expects, when it had one.
    pub(crate) const fn planned_peak_bytes(&self) -> Option<u64> {
        match self {
            Self::Resident { planned_peak_bytes } => *planned_peak_bytes,
            Self::Spilled { combine, .. } => combine.planned_peak_bytes,
        }
    }

    /// The scratch a spilled tier writes to.
    pub(crate) const fn scratch(&self) -> Option<&RunScratch> {
        match self {
            Self::Resident { .. } => None,
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
            Self::Resident { .. } => None,
            Self::Spilled { combine, .. } => Some(*combine),
        }
    }

    /// Keep one frame of a set of `set`, as its admission found it: moved in whole on the
    /// resident tier, written to the scratch and mapped back on the spilled one, which hands its
    /// buffers back.
    pub(crate) fn store<B: FrameBuffers>(
        &self,
        buffers: B,
        admitted: Admitted,
        set: SetKind,
    ) -> Result<Stored<B>, FrameStoreError> {
        match self {
            Self::Resident { .. } => {
                let NewFrame { image, quality } = buffers.into_new();
                Ok(Stored {
                    frame: StoredFrame::from_memory(image, quality, admitted),
                    returned: None,
                })
            }
            Self::Spilled { scratch, .. } => {
                let written = FrameWrite {
                    image: buffers.image(),
                    quality: buffers.quality(),
                    set,
                }
                .write(scratch)?;
                Ok(Stored {
                    frame: written.into_frame(admitted),
                    returned: Some(buffers),
                })
            }
        }
    }
}
