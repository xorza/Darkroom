//! [`PipelineFrame`]: the frame carrier the registered-stacking pipeline moves between stages.

use imaginarium::PlanarDesc;

use crate::frame_store::stored_image::StoredImage;
use crate::image::linear_image::LinearImage;
use crate::image::metadata::image_metadata::ImageMetadata;

/// A calibrated frame waiting to be registered, held wherever the memory tier put it.
///
/// The two variants are the whole difference between the all-RAM and the memory-bounded runs:
/// the pipeline body downstream is identical, because the warp reads either from RAM — a spilled
/// frame read back whole first ([`Self::into_resident`]).
#[derive(Debug)]
pub(super) enum PipelineFrame {
    Resident(LinearImage),
    Spilled(StoredImage),
}

impl PipelineFrame {
    /// The frame in RAM, as the warp reads it. A parked frame is read back whole, one pass front
    /// to back over each of its planes: the warp then reads in whatever order its transform asks —
    /// a column per output row for a quarter turn — from memory the plan already charges for its
    /// source, not by faulting pages of a map the plan does not count.
    pub(super) fn into_resident(self) -> LinearImage {
        match self {
            Self::Resident(image) => image,
            Self::Spilled(stored) => stored.into_image(None),
        }
    }

    pub(super) const fn metadata(&self) -> &ImageMetadata {
        match self {
            Self::Resident(image) => &image.metadata,
            Self::Spilled(stored) => &stored.metadata,
        }
    }

    pub(super) fn desc(&self) -> PlanarDesc {
        match self {
            Self::Resident(image) => image.desc(),
            Self::Spilled(stored) => stored.desc,
        }
    }
}

#[cfg(test)]
mod tests;
