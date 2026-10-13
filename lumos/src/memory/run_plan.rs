//! [`RunPlan`]: the tier one run keeps its frames in, and how wide each of its stages runs.

use arrayvec::ArrayVec;

use crate::memory::budget::Budget;
use crate::memory::stage::{Placement, Stage, Width};

/// The most stages a run has: preparing, warping, normalizing and combining.
const MAX_STAGES: usize = 4;

/// The tier one run keeps its frames in, and how wide each of its stages runs under its budget —
/// the one rule every entry decides with. A run's last stage is its combine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RunPlan {
    pub(crate) resident: bool,
    widths: ArrayVec<Width, MAX_STAGES>,
}

impl RunPlan {
    /// Plan `frame_count` frames through `stages` under `budget`.
    ///
    /// The run stays resident when every stage's narrowest step fits beside the whole set as that
    /// stage keeps it: each frame at the larger of what the stage before kept of it and what this
    /// one keeps, since frames not yet through are still as they came. Otherwise every frame lies
    /// on disk between its stages, and only the frames in work are in memory. Each stage then runs
    /// as wide as what is left allows.
    pub(crate) fn of(frame_count: usize, stages: &[Stage], budget: Budget) -> Self {
        debug_assert!(frame_count > 0, "a run plans one frame at least");
        let kept_sets: ArrayVec<u64, MAX_STAGES> = stages
            .iter()
            .scan(0, |before, stage| {
                let set = ((*before).max(stage.kept) as u64).saturating_mul(frame_count as u64);
                *before = stage.kept;
                Some(set)
            })
            .collect();
        let resident = stages.iter().zip(&kept_sets).all(|(stage, &kept_set)| {
            stage
                .width(budget, frame_count, Placement::Resident { kept_set })
                .overcommit_bytes
                == 0
        });
        let widths = stages
            .iter()
            .zip(&kept_sets)
            .map(|(stage, &kept_set)| {
                let placement = if resident {
                    Placement::Resident { kept_set }
                } else {
                    Placement::Spilled
                };
                stage.width(budget, frame_count, placement)
            })
            .collect();
        Self { resident, widths }
    }

    /// The units stage `stage` runs at once.
    pub(crate) fn width(&self, stage: usize) -> usize {
        self.widths[stage].units
    }

    /// The most bytes the stages before the last — the combine, which plans its own rows again
    /// once it sees the frames — hold past the budget: zero for a resident run, which fits whole.
    pub(crate) fn prior_overcommit(&self) -> u64 {
        self.widths.split_last().map_or(0, |(_, prior)| {
            prior
                .iter()
                .map(|width| width.overcommit_bytes)
                .max()
                .unwrap_or(0)
        })
    }
}
