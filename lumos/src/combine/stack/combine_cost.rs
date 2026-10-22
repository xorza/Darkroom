//! [`CombineCost`]: what a combine holds, priced once for the plan and the frame cache alike.

use crate::image::image_dimensions::ImageDimensions;
use crate::memory::stage::{Stage, Units};
use crate::stack_product::quality_planes::QualityPlanes;

/// The chunk copies a spilled combine holds at once: the chunk it reduces and the next one, read
/// while it does.
const CHUNK_COPIES: usize = 2;

/// What a combine of `dimensions` holds: the output planes `quality` asks for, its
/// `coverage_planes` and `flag_planes`, throughout; and on the spill tier, for each output row,
/// [`CHUNK_COPIES`] copies of every frame's row, `frame_pixel_bytes` a pixel over all the frames.
/// A resident combine reads its frames where they lie, which [`Units::Rows`] charges nothing for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct CombineCost {
    pub(crate) dimensions: ImageDimensions,
    pub(crate) quality: QualityPlanes,
    pub(crate) coverage_planes: usize,
    pub(crate) flag_planes: usize,
    pub(crate) frame_pixel_bytes: usize,
}

impl CombineCost {
    /// The combine as a stage of rows, each frame kept at `kept`.
    pub(crate) const fn stage(self, kept: usize) -> Stage {
        let Self {
            dimensions,
            quality,
            coverage_planes,
            flag_planes,
            frame_pixel_bytes,
        } = self;
        Stage {
            kept,
            held: quality.pixel_bytes(dimensions.channels(), coverage_planes, flag_planes)
                * dimensions.pixel_count(),
            in_flight: CHUNK_COPIES * frame_pixel_bytes * dimensions.width(),
            units: Units::Rows(dimensions.height()),
        }
    }
}
