//! Results and failures from registered stacking pipelines.

use std::path::PathBuf;

use crate::calibration_masters::cosmic_ray::error::UnknownAdcStep;
use crate::calibration_masters::error::CalibrationError;
use crate::combine::error::Error as StackError;
use crate::error::InvalidConfigField;
use crate::frame_store::error::FrameStoreError;
use crate::io::image::error::ImageError;
use crate::pipeline::frame_registration::FrameRegistration;
use crate::stack_product::StackProduct;
use crate::star_detection::detector::Diagnostics;

/// Registration bookkeeping for an aligned stack.
#[derive(Debug, Clone)]
pub struct AlignmentSummary {
    /// Index into the input of the alignment reference frame.
    pub reference: usize,
    /// How each light registered, in input order: the reference, a warp with its fit, or why it
    /// was dropped.
    pub frames: Vec<FrameRegistration>,
}

impl AlignmentSummary {
    /// The number of frames combined into the stack, the reference included.
    pub fn registered(&self) -> usize {
        self.frames
            .iter()
            .filter(|frame| frame.is_stacked())
            .count()
    }

    /// The input indices dropped because registration failed, ascending.
    pub fn dropped(&self) -> Vec<usize> {
        self.frames
            .iter()
            .enumerate()
            .filter(|(_, frame)| !frame.is_stacked())
            .map(|(index, _)| index)
            .collect()
    }
}

/// Outcome of a registered stack.
#[derive(Debug)]
pub struct AlignStackResult {
    /// The combined image and its ancillary per-pixel science planes.
    pub product: StackProduct,
    /// Reference selection and frame registration outcome.
    pub alignment: AlignmentSummary,
    /// Per-frame star-detection funnel, in input order — every frame the pipeline detected on,
    /// including those registration later dropped, so an index here matches an input index.
    pub detection: Vec<Diagnostics>,
}

impl AlignStackResult {
    pub(crate) const fn from_product(
        product: StackProduct,
        reference: usize,
        frames: Vec<FrameRegistration>,
        detection: Vec<Diagnostics>,
    ) -> Self {
        Self {
            product,
            alignment: AlignmentSummary { reference, frames },
            detection,
        }
    }
}

/// Failures from calibrated-image and RAW registered stacking.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("no light frames provided")]
    NoFrames,
    #[error("stacking cancelled")]
    Cancelled,
    #[error("failed to load light frame '{path}': {source}")]
    Load {
        path: PathBuf,
        /// Boxed because an inline [`ImageError`] is 96 bytes and would size
        /// the whole enum on its own. The pipeline returns `Result<(), Error>`
        /// and `Result<usize, Error>`, where that makes the failure case set
        /// what every success costs to return.
        #[source]
        source: Box<ImageError>,
    },
    #[error("reference index {index} out of range ({count} frames)")]
    ReferenceOutOfRange { index: usize, count: usize },
    #[error("reference frame {index} has only {found} stars (need {required})")]
    ReferenceInsufficientStars {
        index: usize,
        found: usize,
        required: usize,
    },
    #[error("all {count} non-reference frames failed to register")]
    AllFramesDropped { count: usize },
    #[error(transparent)]
    Calibration(#[from] CalibrationError),
    /// Both config variants carry the same payload, so neither derives `From` — a bare `?` would
    /// have to guess which config the field came from.
    #[error("invalid star-detection configuration: {0}")]
    DetectionConfig(InvalidConfigField),
    #[error("invalid registration configuration: {0}")]
    RegistrationConfig(InvalidConfigField),
    #[error("invalid cosmic-ray configuration: {0}")]
    CosmicRayConfig(InvalidConfigField),
    #[error("cannot reject cosmic rays in light frame '{path}': {source}")]
    CosmicRay {
        path: PathBuf,
        #[source]
        source: UnknownAdcStep,
    },
    #[error(transparent)]
    FrameStore(#[from] FrameStoreError),
    #[error(transparent)]
    Stack(StackError),
}

/// A combine failure arrives at the one path a caller matches it on: a cancel, an empty set or a
/// frame-store failure as the pipeline's own, everything else as the combine's.
impl From<StackError> for Error {
    fn from(error: StackError) -> Self {
        match error {
            StackError::Cancelled => Self::Cancelled,
            StackError::NoFrames => Self::NoFrames,
            StackError::FrameStore(error) => Self::FrameStore(error),
            error => Self::Stack(error),
        }
    }
}
