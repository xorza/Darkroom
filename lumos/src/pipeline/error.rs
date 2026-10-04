//! [`AlignStackError`]: the failures of calibrated-image and RAW registered stacking.

use std::path::PathBuf;

use crate::calibration_masters::cosmic_ray::error::UnknownAdcStep;
use crate::calibration_masters::error::CalibrationError;
use crate::combine::error::StackError;
use crate::error::InvalidConfigField;
use crate::frame_store::error::FrameStoreError;
use crate::io::image::error::ImageError;

/// Failures from calibrated-image and RAW registered stacking.
#[derive(Debug, thiserror::Error)]
pub enum AlignStackError {
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
impl From<StackError> for AlignStackError {
    fn from(error: StackError) -> Self {
        match error {
            StackError::Cancelled => Self::Cancelled,
            StackError::NoFrames => Self::NoFrames,
            StackError::FrameStore(error) => Self::FrameStore(error),
            error => Self::Stack(error),
        }
    }
}
