//! Results and failures from registered stacking pipelines.

use std::path::PathBuf;

use crate::error::InvalidConfigField;
use crate::io::image::error::ImageError;
use crate::stacking::calibration_masters::CalibrationError;
use crate::stacking::combine::error::Error as StackError;
use crate::stacking::stack_product::StackProduct;
use crate::stacking::star_detection::detector::Diagnostics;

/// Registration bookkeeping for an aligned stack.
#[derive(Debug)]
pub struct AlignmentSummary {
    /// Index into the input of the alignment reference frame.
    pub reference: usize,
    /// Number of frames combined into the stack.
    pub registered: usize,
    /// Input indices dropped because registration failed, ascending.
    pub dropped: Vec<usize>,
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
    pub(crate) fn from_product(
        product: StackProduct,
        reference: usize,
        registered: usize,
        dropped: Vec<usize>,
        detection: Vec<Diagnostics>,
    ) -> Self {
        Self {
            product,
            alignment: AlignmentSummary {
                reference,
                registered,
                dropped,
            },
            detection,
        }
    }
}

/// Failures from calibrated-image and RAW registered stacking.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("no light frames provided")]
    NoFrames,
    #[error("failed to load light frame '{path}': {source}")]
    Load {
        path: PathBuf,
        #[source]
        source: ImageError,
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
    #[error(transparent)]
    Stack(#[from] StackError),
}
