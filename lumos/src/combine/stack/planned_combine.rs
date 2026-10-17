//! [`PlannedCombine`]: a combine as a memory plan prices it, before its frames exist.

use crate::combine::frame_read::slots::Slots;
use crate::combine::normalization::Normalization;
use crate::combine::normalization::planned_normalization::{PlannedNormalization, SetCoverage};
use crate::frame_store::stored_layout::StoredLayout;
use crate::image::image_dimensions::ImageDimensions;
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
    pub(crate) dimensions: ImageDimensions,
    pub(crate) layout: StoredLayout,
    pub(crate) slots: Slots,
    pub(crate) coverage: SetCoverage,
    pub(crate) workers: usize,
}

impl PlannedCombine {
    /// The normalization and the combine of `frames`, each kept at `kept`.
    ///
    /// The normalization holds what measuring the frames holds. The combine holds its output
    /// planes throughout — a coverage plane, one per channel when each channel is covered by its
    /// own rule, and the flag plane — and each of its rows reads every frame in its layout. An
    /// upper bound for frames that turn out to carry no quality planes or no flags, which leave
    /// those planes out.
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
                SetCoverage::PerChannel => dimensions.channels(),
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
            Stage {
                kept,
                held: self
                    .quality
                    .pixel_bytes(dimensions.channels(), coverage_planes, 1)
                    * dimensions.pixel_count(),
                in_flight: count * layout.pixel_bytes() * dimensions.width(),
                units: Units::Rows(dimensions.height()),
            },
        ]
    }
}
