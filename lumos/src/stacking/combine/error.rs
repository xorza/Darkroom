//! Error types for stacking operations.

use std::io;
use std::path::PathBuf;

use thiserror::Error;

use crate::error::InvalidConfigField;
use crate::io::image::image_dimensions::ImageDimensions;
use crate::stacking::calibration_masters::CalibrationError;
use crate::stacking::frame_store::{FramePlane, FrameStoreError};

/// Invalid [`crate::StackConfig`] parameters.
///
/// Plain range checks report through [`InvalidConfigField`]; the variants below are the
/// constraints that aren't one — a per-element check and two that span fields.
#[derive(Debug, Error, Clone, PartialEq)]
pub enum StackConfigError {
    #[error(transparent)]
    Field(#[from] InvalidConfigField),

    #[error("manual weight {index} must be finite and non-negative, got {value}")]
    InvalidManualWeight { index: usize, value: f32 },

    #[error("manual weights must contain at least one positive value with a finite sum")]
    InvalidManualWeightSum,

    #[error("manual weight count {actual} does not match frame count {expected}")]
    ManualWeightCountMismatch { expected: usize, actual: usize },

    #[error("small-stack fallback must not use pixel rejection")]
    RejectingSmallNFallback,
}

/// Errors that can occur during stacking operations.
#[derive(Debug, Error)]
pub enum Error {
    #[error(transparent)]
    Config(#[from] StackConfigError),

    #[error(transparent)]
    FrameStore(#[from] FrameStoreError),

    /// A calibration bundle built alongside a stack does not describe one coherent sensor.
    #[error(transparent)]
    Calibration(#[from] CalibrationError),

    #[error("No frames provided for stacking")]
    NoFrames,

    #[error("stacking cancelled")]
    Cancelled,

    #[error("registered frames have no pixels with common valid warp support")]
    NoCommonCoverage,

    #[error("Failed to load image '{path}': {source}")]
    ImageLoad {
        path: PathBuf,
        #[source]
        source: io::Error,
    },

    #[error("Dimension mismatch for frame {index}: expected {expected:?}, got {actual:?}")]
    DimensionMismatch {
        index: usize,
        expected: ImageDimensions,
        actual: ImageDimensions,
    },

    /// A frame already in the frame store does not match the geometry the cache was built for.
    /// Reported as a plane count and sample counts rather than as [`ImageDimensions`] because a
    /// stored plane knows only its length — it has no width or height to report.
    #[error("stored frame {index} has {actual} channel planes, expected {expected}")]
    StoredFrameChannels {
        index: usize,
        expected: usize,
        actual: usize,
    },

    #[error("stored frame {index} {plane} holds {actual} samples, expected {expected}")]
    StoredFramePlaneSamples {
        index: usize,
        plane: FramePlane,
        expected: usize,
        actual: usize,
    },

    #[error("frame {index}, channel {channel}, pixel {pixel} has non-finite image value {value}")]
    NonFiniteImageSample {
        index: usize,
        channel: usize,
        pixel: usize,
        value: f32,
    },

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

    #[error("{plane} for frame {index} has invalid value {value} at pixel {pixel}")]
    InvalidWarpPlaneValue {
        index: usize,
        plane: FramePlane,
        pixel: usize,
        value: f32,
    },
}

#[cfg(test)]
mod tests {
    use crate::stacking::combine::error::*;

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
        let err = Error::NoFrames;
        assert_eq!(err.to_string(), "No frames provided for stacking");
        assert_eq!(
            Error::NoCommonCoverage.to_string(),
            "registered frames have no pixels with common valid warp support"
        );
    }

    #[test]
    fn image_load_error_message() {
        let err = Error::ImageLoad {
            path: PathBuf::from("/path/to/image.fits"),
            source: io::Error::new(io::ErrorKind::NotFound, "file not found"),
        };
        assert!(err.to_string().contains("/path/to/image.fits"));
        assert!(err.to_string().contains("file not found"));
    }

    #[test]
    fn dimension_mismatch_error_message() {
        let err = Error::DimensionMismatch {
            index: 5,
            expected: ImageDimensions::new((100, 100), 3),
            actual: ImageDimensions::new((200, 100), 3),
        };
        let msg = err.to_string();
        assert!(msg.contains("5"));
        assert!(msg.contains("100"));
        assert!(msg.contains("200"));
    }

    #[test]
    fn frame_store_error_is_transparent() {
        let error = Error::from(FrameStoreError::WriteFile {
            path: PathBuf::from("/tmp/cache/frame.bin"),
            source: io::Error::other("disk full"),
        });
        assert_eq!(
            error.to_string(),
            "failed to write frame-store file '/tmp/cache/frame.bin': disk full"
        );
    }

    #[test]
    fn error_is_debug() {
        let err = Error::NoFrames;
        let debug_str = format!("{:?}", err);
        assert!(debug_str.contains("NoFrames"));
    }

    #[test]
    fn error_source_chain() {
        use std::error::Error as StdError;

        let io_err = io::Error::new(io::ErrorKind::NotFound, "underlying error");
        let err = Error::ImageLoad {
            path: PathBuf::from("/test"),
            source: io_err,
        };

        // Verify source() returns the underlying io::Error
        assert!(err.source().is_some());
    }
}
