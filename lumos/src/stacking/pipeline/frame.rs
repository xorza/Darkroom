//! The frame carrier the registered-stacking pipeline moves between stages.

use crate::io::image::image_dimensions::ImageDimensions;
use crate::io::image::image_metadata::ImageMetadata;
use crate::io::image::linear::LinearImage;
use crate::stacking::frame_store::StoredImage;
use crate::stacking::star_detection::detector::Diagnostics;
use crate::stacking::star_detection::star::Star;

/// A calibrated frame waiting to be registered, held wherever the memory tier put it.
///
/// The two variants are the whole difference between the all-RAM and the memory-bounded runs:
/// the pipeline body downstream is identical, because taking the image back out is a move in
/// the resident case and a read from the memory map in the spilled one.
#[derive(Debug)]
pub(crate) enum PipelineFrame {
    Resident(LinearImage),
    Spilled(StoredImage),
}

impl PipelineFrame {
    /// Take the image back. Free for a resident frame — the `LinearImage` moves out untouched —
    /// and a read out of the memory map for a spilled one. Consuming rather than borrowing is
    /// what lets the register/warp stage drop each input as soon as its warped output exists,
    /// so the pipeline never holds the complete input and output sets at once.
    pub(crate) fn into_image(self) -> LinearImage {
        match self {
            Self::Resident(image) => image,
            Self::Spilled(stored) => stored.load(),
        }
    }

    pub(crate) fn metadata(&self) -> &ImageMetadata {
        match self {
            Self::Resident(image) => &image.metadata,
            Self::Spilled(stored) => &stored.metadata,
        }
    }

    pub(crate) fn dimensions(&self) -> ImageDimensions {
        match self {
            Self::Resident(image) => image.dimensions(),
            Self::Spilled(stored) => stored.dimensions,
        }
    }
}

/// One frame whose pixels and detected stars advance through the pipeline together.
#[derive(Debug)]
pub(crate) struct DetectedFrame {
    pub(crate) image: PipelineFrame,
    pub(crate) stars: Vec<Star>,
    /// The detection funnel for this frame, carried through to the caller rather than only logged.
    pub(crate) diagnostics: Diagnostics,
}
