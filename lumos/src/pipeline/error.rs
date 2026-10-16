//! [`AlignStackError`]: the failures of calibrated-image and RAW registered stacking.

use std::path::PathBuf;

use crate::calibration::cosmic_ray::error::CosmicRayError;
use crate::calibration::error::CalibrationError;
use crate::combine::error::{StackConfigError, StackError};
use crate::error::Cancelled;
use crate::error::InvalidConfigField;
use crate::frame_store::error::{FrameStoreError, LoadError};
use crate::ingest::error::AdmissionError;
use crate::io::error::ImageError;

/// Failures from calibrated-image and RAW registered stacking.
#[derive(Debug, thiserror::Error)]
pub enum AlignStackError {
    #[error("no light frames provided")]
    NoFrames,
    #[error("stacking cancelled")]
    Cancelled,
    /// A light that could not be decoded, which [`ImageError`] names. Boxed because an inline
    /// [`ImageError`] is 96 bytes and would size the whole enum on its own. The pipeline returns
    /// `Result<(), Error>` and `Result<usize, Error>`, where that makes the failure case set what
    /// every success costs to return.
    #[error(transparent)]
    Load(Box<ImageError>),
    /// The configuration cannot run over these lights, found before any light is worked.
    #[error("invalid configuration: {0}")]
    Config(#[from] AlignStackConfigError),
    #[error("reference frame {index} has only {found} stars (need {required})")]
    ReferenceInsufficientStars {
        index: usize,
        found: usize,
        required: usize,
    },
    #[error("all {count} non-reference frames failed to register")]
    AllFramesDropped { count: usize },
    #[error("cannot calibrate light frame '{path}': {source}")]
    Calibration {
        path: PathBuf,
        /// Boxed for the reason [`Self::Load`] is.
        #[source]
        source: Box<CalibrationError>,
    },
    #[error("cannot reject cosmic rays in light frame '{path}': {source}")]
    CosmicRay {
        path: PathBuf,
        #[source]
        source: CosmicRayError,
    },
    #[error(transparent)]
    FrameStore(#[from] FrameStoreError),
    /// A light failed the checks every frame of the set passes, whether as it was decoded or when
    /// the survivors were combined.
    #[error(transparent)]
    Admission(AdmissionError),
    #[error(transparent)]
    Stack(StackError),
}

/// Why an [`AlignStackConfig`](crate::AlignStackConfig) cannot run over its lights: a stage's
/// configuration, named by the stage — the stages' fields share names — or a rule across them.
#[derive(Debug, thiserror::Error)]
pub enum AlignStackConfigError {
    #[error("star detection: {0}")]
    Detection(InvalidConfigField),
    #[error("registration: {0}")]
    Registration(InvalidConfigField),
    #[error("warp: {0}")]
    Warp(InvalidConfigField),
    #[error("cosmic-ray rejection: {0}")]
    CosmicRay(InvalidConfigField),
    #[error("combine: {0}")]
    Stack(#[from] StackConfigError),
    #[error("reference index {index} out of range ({count} frames)")]
    ReferenceOutOfRange { index: usize, count: usize },
    /// A cosmic-ray pass asked of lights that are already linear: the pass runs on a mosaic
    /// before its demosaic would smear a hit across pixels, and these lights have none.
    #[error(
        "single-frame cosmic-ray rejection runs on mosaic lights before their demosaic; these \
         lights are already linear, so leave it to stack-time rejection"
    )]
    CosmicRayOnLinearLights,
}

impl From<Cancelled> for AlignStackError {
    fn from(Cancelled: Cancelled) -> Self {
        Self::Cancelled
    }
}

/// A decode stopped by the run's cancel token arrives as the run's own cancel, every other failure
/// as the file's.
impl From<ImageError> for AlignStackError {
    fn from(error: ImageError) -> Self {
        match error {
            ImageError::Cancelled { .. } => Self::Cancelled,
            error => Self::Load(Box::new(error)),
        }
    }
}

impl From<LoadError> for AlignStackError {
    fn from(error: LoadError) -> Self {
        match error {
            LoadError::Decode(error) => error.into(),
            LoadError::Store(error) => error.into(),
        }
    }
}

impl From<AdmissionError> for AlignStackError {
    fn from(error: AdmissionError) -> Self {
        match error {
            AdmissionError::Cancelled => Self::Cancelled,
            error => Self::Admission(error),
        }
    }
}

/// A combine failure arrives at the one path a caller matches it on: a cancel, an empty set or a
/// frame-store failure as the pipeline's own, everything else as the combine's.
impl From<StackError> for AlignStackError {
    fn from(error: StackError) -> Self {
        match error {
            StackError::Cancelled => Self::Cancelled,
            StackError::NoFrames => Self::NoFrames,
            StackError::FrameStore(error) => Self::FrameStore(error),
            StackError::Admission(error) => Self::Admission(error),
            StackError::Config(error) => Self::Config(error.into()),
            error => Self::Stack(error),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::io;
    use std::path::PathBuf;

    use crate::io::error::ImageError;
    use crate::pipeline::error::AlignStackError;

    /// A decode the run's token stopped is the run's cancel, and a file failure names its path
    /// once, as the decoder's error already does.
    #[test]
    fn a_decode_error_converts_by_its_case() {
        let path = PathBuf::from("/lights/l1.fits");
        let cancelled = AlignStackError::from(ImageError::Cancelled { path: path.clone() });
        assert!(
            matches!(cancelled, AlignStackError::Cancelled),
            "{cancelled:?}"
        );

        let missing = AlignStackError::from(ImageError::Io {
            path,
            source: io::Error::from(io::ErrorKind::NotFound),
        });
        assert!(matches!(missing, AlignStackError::Load(_)), "{missing:?}");
        assert_eq!(missing.to_string().matches("/lights/l1.fits").count(), 1);
    }
}
