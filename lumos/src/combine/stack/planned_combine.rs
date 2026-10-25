//! [`PlannedCombine`]: a combine as a memory plan prices it, before its frames exist.

use imaginarium::PlanarDesc;

use crate::combine::frame_read::slots::Slots;
use crate::combine::normalization::Normalization;
use crate::combine::normalization::planned_normalization::{PlannedNormalization, SetCoverage};
use crate::combine::stack::combine_cost::CombineCost;
use crate::frame_store::stored_layout::StoredLayout;
use crate::memory::stage::{Stage, Units};
use crate::stack_product::quality_planes::QualityPlanes;

/// The settings a combine is priced by: its normalization, which holds the frames' samples, and
/// its output planes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct PlannedCombine {
    pub(crate) normalization: Normalization,
    pub(crate) quality: QualityPlanes,
}

/// The frames a planned combine reads: `count` frames of `dimensions` stored in `layout`, read in
/// `slots`, covering the pixels as `coverage` says, on `workers`.
#[derive(Debug, Clone, Copy)]
pub(crate) struct PlannedFrames {
    pub(crate) count: usize,
    pub(crate) dimensions: PlanarDesc,
    pub(crate) layout: StoredLayout,
    pub(crate) slots: Slots,
    pub(crate) coverage: SetCoverage,
    pub(crate) workers: usize,
}

impl PlannedCombine {
    /// The normalization and the combine of `frames`, each kept at `kept`.
    ///
    /// The normalization holds what measuring the frames holds. The combine is the
    /// [`CombineCost`] of a coverage plane, one per channel when each channel is covered by its
    /// own rule, the flag plane, and every frame in its layout. An upper bound for frames that turn
    /// out to carry no quality planes or no flags, which leave those planes out.
    pub(crate) fn stages(self, frames: PlannedFrames, kept: usize) -> [Stage; 2] {
        let PlannedFrames {
            count,
            dimensions,
            layout,
            slots,
            coverage,
            workers,
        } = frames;
        let coverage_planes = usize::from(self.quality.coverage)
            * match coverage {
                SetCoverage::Common => 1,
                SetCoverage::PerChannel => dimensions.channel_count(),
            };
        [
            Stage {
                kept,
                held: PlannedNormalization {
                    frame_count: count,
                    dimensions,
                    slots,
                    normalization: self.normalization,
                    coverage,
                    threads: workers,
                }
                .heap_bytes(),
                in_flight: 0,
                units: Units::Whole,
            },
            CombineCost {
                dimensions,
                quality: self.quality,
                coverage_planes,
                flag_planes: 1,
                frame_pixel_bytes: count * layout.pixel_bytes(),
                frame_count: count,
                workers,
            }
            .stage(kept),
        ]
    }
}
