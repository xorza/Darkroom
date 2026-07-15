//! Error types for stacking operations.

use std::io;
use std::path::PathBuf;

use thiserror::Error;

use crate::io::astro_image::ImageDimensions;

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

    #[error("GESD low_relaxation must be finite and at least 1, got {value}")]
    InvalidGesdLowRelaxation { value: f32 },

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

    #[error("No frames provided for stacking")]
    NoFrames,

    #[error("stacking cancelled")]
    Cancelled,

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
        "Coverage dimensions for frame {index} do not match: expected {expected_width}x{expected_height}, got {actual_width}x{actual_height}"
    )]
    CoverageDimensionMismatch {
        index: usize,
        expected_width: usize,
        expected_height: usize,
        actual_width: usize,
        actual_height: usize,
    },

    #[error("Failed to create cache directory '{path}': {source}")]
    CreateCacheDir {
        path: PathBuf,
        #[source]
        source: io::Error,
    },

    #[error("Failed to create cache file '{path}': {source}")]
    CreateCacheFile {
        path: PathBuf,
        #[source]
        source: io::Error,
    },

    #[error("Failed to write cache file '{path}': {source}")]
    WriteCacheFile {
        path: PathBuf,
        #[source]
        source: io::Error,
    },

    #[error("Failed to open cache file '{path}': {source}")]
    OpenCacheFile {
        path: PathBuf,
        #[source]
        source: io::Error,
    },

    #[error("Failed to memory-map cache file '{path}': {source}")]
    MmapCacheFile {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_no_frames_error_message() {
        let err = Error::NoFrames;
        assert_eq!(err.to_string(), "No frames provided for stacking");
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
    fn test_create_cache_dir_error_message() {
        let err = Error::CreateCacheDir {
            path: PathBuf::from("/tmp/cache"),
            source: io::Error::new(io::ErrorKind::PermissionDenied, "permission denied"),
        };
        assert!(err.to_string().contains("/tmp/cache"));
        assert!(err.to_string().contains("permission denied"));
    }

    #[test]
    fn test_create_cache_file_error_message() {
        let err = Error::CreateCacheFile {
            path: PathBuf::from("/tmp/cache/frame.bin"),
            source: io::Error::new(io::ErrorKind::PermissionDenied, "permission denied"),
        };
        assert!(err.to_string().contains("/tmp/cache/frame.bin"));
    }

    #[test]
    fn test_write_cache_file_error_message() {
        let err = Error::WriteCacheFile {
            path: PathBuf::from("/tmp/cache/frame.bin"),
            source: io::Error::other("disk full"),
        };
        assert!(err.to_string().contains("/tmp/cache/frame.bin"));
        assert!(err.to_string().contains("disk full"));
    }

    #[test]
    fn test_open_cache_file_error_message() {
        let err = Error::OpenCacheFile {
            path: PathBuf::from("/tmp/cache/frame.bin"),
            source: io::Error::new(io::ErrorKind::NotFound, "not found"),
        };
        assert!(err.to_string().contains("/tmp/cache/frame.bin"));
    }

    #[test]
    fn test_mmap_cache_file_error_message() {
        let err = Error::MmapCacheFile {
            path: PathBuf::from("/tmp/cache/frame.bin"),
            source: io::Error::other("mmap failed"),
        };
        assert!(err.to_string().contains("/tmp/cache/frame.bin"));
        assert!(err.to_string().contains("mmap failed"));
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
