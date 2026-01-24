//! Error types for stacking operations.

use std::io;
use std::path::PathBuf;

use thiserror::Error;

use crate::astro_image::ImageDimensions;
use crate::stacking::FrameType;

/// Errors that can occur during stacking operations.
#[derive(Debug, Error)]
pub enum Error {
    #[error("No paths provided for stacking")]
    NoPaths,

    #[error("Failed to load image '{path}': {source}")]
    ImageLoad {
        path: PathBuf,
        #[source]
        source: io::Error,
    },

    #[error(
        "Dimension mismatch for {frame_type} frame {index}: expected {expected:?}, got {actual:?}"
    )]
    DimensionMismatch {
        frame_type: FrameType,
        index: usize,
        expected: ImageDimensions,
        actual: ImageDimensions,
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
    fn test_no_paths_error_message() {
        let err = Error::NoPaths;
        assert_eq!(err.to_string(), "No paths provided for stacking");
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
            frame_type: FrameType::Dark,
            index: 5,
            expected: ImageDimensions {
                width: 100,
                height: 100,
                channels: 3,
            },
            actual: ImageDimensions {
                width: 200,
                height: 100,
                channels: 3,
            },
        };
        let msg = err.to_string();
        assert!(msg.contains("dark"));
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
        let err = Error::NoPaths;
        let debug_str = format!("{:?}", err);
        assert!(debug_str.contains("NoPaths"));
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
