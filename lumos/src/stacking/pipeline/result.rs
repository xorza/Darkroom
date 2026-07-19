//! Results and failures from registered stacking pipelines.

use std::path::PathBuf;

use crate::io::astro_image::error::ImageError;
use crate::stacking::calibration_masters::CalibrationError;
use crate::stacking::combine::error::Error as StackError;
use crate::stacking::product::StackProduct;
use crate::stacking::star_detection::error::StarDetectionConfigError;

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
}

impl AlignStackResult {
    pub(crate) fn from_product(
        product: StackProduct,
        reference: usize,
        registered: usize,
        dropped: Vec<usize>,
    ) -> Self {
        Self {
            product,
            alignment: AlignmentSummary {
                reference,
                registered,
                dropped,
            },
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
    #[error(transparent)]
    DetectionConfig(#[from] StarDetectionConfigError),
    #[error(transparent)]
    Stack(#[from] StackError),
}
