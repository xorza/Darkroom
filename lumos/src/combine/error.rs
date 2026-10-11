//! StackError types for stacking operations.

use thiserror::Error;

use crate::calibration_masters::error::SubtractionError;
use crate::calibration_masters::master_role::MasterRole;
use crate::error::{FrameDimensionMismatch, InvalidConfigField};
use crate::frame_store::error::{ConditionMismatch, FrameStoreError};
use crate::frame_store::frame_quality::FramePlane;
use crate::ingest::error::AdmissionError;
use crate::io::cancelled::Cancelled;
use crate::io::image::error::ImageError;

/// Invalid [`crate::StackConfig`] parameters.
///
/// Plain range checks report through [`InvalidConfigField`]; the variants below are the
/// constraints that aren't one — a per-element check and two that span fields.
#[derive(Debug, Error, Clone, PartialEq)]
pub enum StackConfigError {
    #[error(transparent)]
    Field(#[from] InvalidConfigField),

    #[error(
        "manual weight {index} must be finite and positive, got {value}: drop a frame from the set rather than weigh it 0"
    )]
    InvalidManualWeight { index: usize, value: f32 },

    #[error("manual weights must contain at least one value, with a finite sum")]
    InvalidManualWeightSum,

    #[error("manual weight count {actual} does not match frame count {expected}")]
    ManualWeightCountMismatch { expected: usize, actual: usize },

    #[error("small-stack fallback must not use pixel rejection")]
    RejectingSmallNFallback,
}

/// Errors that can occur during stacking operations.
#[derive(Debug, Error)]
pub enum StackError {
    #[error(transparent)]
    Config(#[from] StackConfigError),

    #[error(transparent)]
    FrameStore(#[from] FrameStoreError),

    #[error("No frames provided for stacking")]
    NoFrames,

    #[error("stacking cancelled")]
    Cancelled,

    #[error("registered frames have no pixels with common valid warp support")]
    NoCommonCoverage,

    /// Multiplicative normalization divides by each frame's median, and this one is not positive.
    #[error(
        "frame {index} has median {median} in slot {slot}; a multiplicative normalization needs a positive one"
    )]
    NonPositiveMedian {
        index: usize,
        /// A channel, or a colour of a mosaic.
        slot: usize,
        median: f32,
    },

    /// A normalization that scales each frame by its own spread found this one's spread no larger
    /// than its white noise: the slot holds no signal structure to scale by.
    #[error(
        "frame {index} has spread {spread} in slot {slot}, no larger than its white noise {noise}; \
         it holds no signal to scale a normalization by"
    )]
    NoSignalScale {
        index: usize,
        /// A channel, or a colour of a mosaic.
        slot: usize,
        spread: f32,
        noise: f32,
    },

    /// The master a calibration stack subtracts from every frame cannot be taken from them.
    #[error(transparent)]
    Subtraction(#[from] SubtractionError),

    /// The frames of a master whose dark signal is one exposure's at one temperature — a dark or a
    /// flat-dark — were not all taken under one: the master would state one frame's conditions and
    /// hold the majority's signal.
    #[error("the {role} frames must share one exposure and one temperature: {source}")]
    MasterConditions {
        role: MasterRole,
        source: ConditionMismatch,
    },

    /// `Weighting::Noise` weighs each frame by its inverse noise variance, and this frame measured
    /// none: only synthetic data has no noise at all.
    #[error("frame {index} has no measured noise to weight by; use Equal or Manual weighting")]
    NoNoiseToWeigh { index: usize },

    /// A file that could not be decoded, held as the decoder's own error — which already names the
    /// path, and stays matchable on *which* decode failed. Wrapping it in a variant of our own
    /// printed the path twice and flattened the cause into an `io::Error`. A decode that was
    /// cancelled arrives as [`Self::Cancelled`], never here.
    #[error(transparent)]
    ImageLoad(#[from] ImageError),

    #[error(
        "{plane} dimensions for frame {index} do not match: expected {expected_width}x{expected_height}, got {actual_width}x{actual_height}"
    )]
    WarpPlaneDimensionMismatch {
        index: usize,
        plane: FramePlane,
        expected_width: usize,
        expected_height: usize,
        actual_width: usize,
        actual_height: usize,
    },

    /// A frame failed the checks every frame of a set passes before it is combined.
    #[error(transparent)]
    Admission(AdmissionError),
}

impl From<FrameDimensionMismatch> for StackError {
    fn from(mismatch: FrameDimensionMismatch) -> Self {
        Self::Admission(AdmissionError::DimensionMismatch(mismatch))
    }
}

/// A frame refused at admission is the run's refusal, a cancel inside the checks the run's cancel.
impl From<AdmissionError> for StackError {
    fn from(error: AdmissionError) -> Self {
        match error {
            AdmissionError::Cancelled => Self::Cancelled,
            error => Self::Admission(error),
        }
    }
}

impl From<Cancelled> for StackError {
    fn from(Cancelled: Cancelled) -> Self {
        Self::Cancelled
    }
}

#[cfg(test)]
mod tests {
    use std::error::Error as _;
    use std::io;
    use std::path::PathBuf;

    use crate::combine::error::*;
    use crate::io::image::image_dimensions::ImageDimensions;

    #[test]
    fn each_plane_carries_its_own_range_and_label() {
        for (plane, value, accepted) in [
            // Coverage is the fraction of the pixel that had support.
            (FramePlane::Coverage, 0.0, true),
            (FramePlane::Coverage, 1.0, true),
            (FramePlane::Coverage, -0.001, false),
            (FramePlane::Coverage, 1.001, false),
            // Confidence is an interpolation weight — non-negative, no upper bound.
            (FramePlane::Confidence, 0.0, true),
            (FramePlane::Confidence, 5.0, true),
            (FramePlane::Confidence, -0.001, false),
            // A calibrated channel may sit below zero once the dark is subtracted.
            (FramePlane::Channel, -1000.0, true),
            (FramePlane::Channel, 1000.0, true),
        ] {
            assert_eq!(plane.accepts(value), accepted, "{plane} accepting {value}");
        }

        for plane in [
            FramePlane::Channel,
            FramePlane::Coverage,
            FramePlane::Confidence,
        ] {
            assert!(!plane.accepts(f32::NAN), "{plane} accepted NaN");
            assert!(!plane.accepts(f32::INFINITY), "{plane} accepted infinity");
        }

        // The labels reach users through the error messages below.
        assert_eq!(FramePlane::Channel.to_string(), "a channel");
        assert_eq!(FramePlane::Coverage.to_string(), "coverage");
        assert_eq!(FramePlane::Confidence.to_string(), "confidence");
    }

    #[test]
    fn no_frames_error_message() {
        let err = StackError::NoFrames;
        assert_eq!(err.to_string(), "No frames provided for stacking");
        assert_eq!(
            StackError::NoCommonCoverage.to_string(),
            "registered frames have no pixels with common valid warp support"
        );
    }

    /// A load failure travels as the decoder's own error: the path appears once, the variant stays
    /// matchable on which decode failed, and `source()` reaches the real cause. Wrapping it in a
    /// `{ path, source: io::Error }` of our own printed "Failed to load image '{p}': Failed to read
    /// file '{p}': …" and flattened the `ImageError` into a string.
    #[test]
    fn a_load_failure_states_the_path_once_and_stays_typed() {
        let path = PathBuf::from("/path/to/image.fits");
        let error = StackError::from(ImageError::Io {
            path: path.clone(),
            source: io::Error::new(io::ErrorKind::NotFound, "file not found"),
        });

        let message = error.to_string();
        assert_eq!(
            message,
            "Failed to read file '/path/to/image.fits': file not found"
        );
        assert_eq!(message.matches("/path/to/image.fits").count(), 1);
        assert!(matches!(
            error,
            StackError::ImageLoad(ImageError::Io { .. })
        ));

        let source = error.source().expect("the decode's own cause");
        assert!(source.downcast_ref::<io::Error>().is_some(), "{source}");
    }

    /// The variants that carry a shared payload print exactly what the payload prints — the wording
    /// lives with the type that owns it, not copied into each subsystem's error.
    #[test]
    fn shared_payloads_are_reported_transparently() {
        let store = StackError::from(FrameStoreError::WriteFile {
            path: PathBuf::from("/tmp/cache/frame.bin"),
            source: io::Error::other("disk full"),
        });
        assert_eq!(
            store.to_string(),
            "failed to write frame-store file '/tmp/cache/frame.bin': disk full"
        );

        let mismatch = FrameDimensionMismatch::check(
            5,
            ImageDimensions::new((100, 100), 3),
            ImageDimensions::new((200, 100), 3),
        )
        .unwrap_err();
        assert_eq!(
            StackError::from(mismatch).to_string(),
            "frame 5 is 200x100x3, expected 100x100x3"
        );
    }

    #[test]
    fn error_is_debug() {
        let err = StackError::NoFrames;
        let debug_str = format!("{err:?}");
        assert!(debug_str.contains("NoFrames"));
    }
}
