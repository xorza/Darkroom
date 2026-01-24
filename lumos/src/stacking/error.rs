//! Error types for stacking operations.

use std::io;
use std::path::PathBuf;

use thiserror::Error;

use crate::astro_image::ImageDimensions;
use crate::stacking::FrameType;

/// Errors that can occur during stacking operations.
#[derive(Debug, Error)]
pub enum StackError {
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
