//! Error types for drizzle stacking.

use std::io;
use std::path::PathBuf;

use thiserror::Error;

use crate::io::astro_image::ImageDimensions;

/// Invalid [`crate::DrizzleConfig`] parameters.
#[derive(Debug, Error, Clone, PartialEq)]
pub enum DrizzleConfigError {
    #[error("scale must be finite and positive, got {value}")]
    InvalidScale { value: f32 },

    #[error("pixfrac must be finite and between 0 and 1, got {value}")]
    InvalidPixfrac { value: f32 },

    #[error("fill_value must be finite, got {value}")]
    InvalidFillValue { value: f32 },

    #[error("min_coverage must be finite and between 0 and 1, got {value}")]
    InvalidMinCoverage { value: f32 },

    #[error("Lanczos requires scale=1 and pixfrac=1, got scale={scale}, pixfrac={pixfrac}")]
    InvalidLanczosSampling { scale: f32, pixfrac: f32 },
}

/// Errors that can occur during drizzle stacking.
#[derive(Debug, Error)]
pub enum DrizzleError {
    #[error(transparent)]
    Config(#[from] DrizzleConfigError),

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

    #[error("invalid drizzle input dimensions {width}x{height} with {channels} channels")]
    InvalidInputDimensions {
        width: usize,
        height: usize,
        channels: usize,
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
            expected: ImageDimensions::new((100, 100), 3),
            actual: ImageDimensions::new((200, 100), 3),
        };
        let msg = err.to_string();
        assert!(msg.contains("frame 2"));
        assert!(msg.contains("100"));
        assert!(msg.contains("200"));
    }
}
