//! Error types for drizzle stacking.

use std::io;
use std::path::PathBuf;

use thiserror::Error;

use crate::astro_image::ImageDimensions;

/// Errors that can occur during drizzle stacking.
#[derive(Debug, Error)]
pub enum DrizzleError {
    #[error("No frames provided for drizzle")]
    NoFrames,

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
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_no_frames_message() {
        assert_eq!(
            DrizzleError::NoFrames.to_string(),
            "No frames provided for drizzle"
        );
    }

    #[test]
    fn test_dimension_mismatch_message() {
        let err = DrizzleError::DimensionMismatch {
            index: 2,
            expected: ImageDimensions::new(100, 100, 3),
            actual: ImageDimensions::new(200, 100, 3),
        };
        let msg = err.to_string();
        assert!(msg.contains("frame 2"));
        assert!(msg.contains("100"));
        assert!(msg.contains("200"));
    }
}
