//! Error types for stacking operations.

use std::io;
use std::path::PathBuf;

use thiserror::Error;

use crate::io::astro_image::ImageDimensions;
use crate::stacking::frame_store::FrameStoreError;

/// Invalid [`crate::StackConfig`] parameters.
#[derive(Debug, Error, Clone, PartialEq)]
pub enum StackConfigError {
    #[error("sigma_low must be finite and positive, got {value}")]
    InvalidSigmaLow { value: f32 },

    #[error("sigma_high must be finite and positive, got {value}")]
    InvalidSigmaHigh { value: f32 },

    #[error("max_iterations must be at least 1")]
    ZeroMaxIterations,

    #[error("low_percentile must be finite and between 0 and 50, got {value}")]
    InvalidLowPercentile { value: f32 },

    #[error("high_percentile must be finite and between 0 and 50, got {value}")]
    InvalidHighPercentile { value: f32 },

    #[error("low_percentile + high_percentile must be less than 100, got {total}")]
    InvalidTotalPercentile { total: f32 },

    #[error("GESD alpha must be finite and between 0 and 1, got {value}")]
    InvalidGesdAlpha { value: f32 },

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

    #[error(
        "{plane} dimensions for frame {index} do not match: expected {expected_width}x{expected_height}, got {actual_width}x{actual_height}"
    )]
    WarpPlaneDimensionMismatch {
        index: usize,
        plane: &'static str,
        expected_width: usize,
        expected_height: usize,
        actual_width: usize,
        actual_height: usize,
    },

    #[error("{plane} for frame {index} has invalid value {value} at pixel {pixel}")]
    InvalidWarpPlaneValue {
        index: usize,
        plane: &'static str,
        pixel: usize,
        value: f32,
    },
}

#[cfg(test)]
mod tests {
    use crate::stacking::combine::error::*;

    #[test]
    fn test_no_frames_error_message() {
        let err = Error::NoFrames;
        assert_eq!(err.to_string(), "No frames provided for stacking");
        assert_eq!(
            Error::NoCommonCoverage.to_string(),
            "registered frames have no pixels with common valid warp support"
        );
    }

    #[test]
    fn test_image_load_error_message() {
        let err = Error::ImageLoad {
            path: PathBuf::from("/path/to/image.fits"),
            source: io::Error::new(io::ErrorKind::NotFound, "file not found"),
        };
        assert!(err.to_string().contains("/path/to/image.fits"));
        assert!(err.to_string().contains("file not found"));
    }

    #[test]
    fn test_dimension_mismatch_error_message() {
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
    fn test_error_is_debug() {
        let err = Error::NoFrames;
        let debug_str = format!("{:?}", err);
        assert!(debug_str.contains("NoFrames"));
    }

    #[test]
    fn test_error_source_chain() {
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
