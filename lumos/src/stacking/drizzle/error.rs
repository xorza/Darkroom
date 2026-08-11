//! Error types for drizzle stacking.

use thiserror::Error;

use crate::error::{FrameDimensionMismatch, InvalidConfigField};
use crate::io::image::error::ImageError;

/// Invalid [`crate::DrizzleConfig`] parameters.
///
/// Plain range checks report through [`InvalidConfigField`]; the variant below constrains two
/// fields against the kernel, so it isn't one.
#[derive(Debug, Error, Clone, PartialEq)]
pub enum DrizzleConfigError {
    #[error(transparent)]
    Field(#[from] InvalidConfigField),

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

    #[error("drizzle cancelled")]
    Cancelled,

    /// A file that could not be decoded — see [`StackError::ImageLoad`](crate::StackError::ImageLoad)
    /// for why the decoder's own error travels rather than a wrapper. A cancelled decode arrives as
    /// [`Self::Cancelled`], never here.
    #[error(transparent)]
    ImageLoad(#[from] ImageError),

    #[error(transparent)]
    DimensionMismatch(#[from] FrameDimensionMismatch),

    #[error("drizzle frame {index} weight must be finite and non-negative, got {value}")]
    InvalidFrameWeight { index: usize, value: f32 },

    #[error(
        "pixel weight map dimensions for drizzle frame {index} do not match: expected {expected_width}x{expected_height}, got {actual_width}x{actual_height}"
    )]
    PixelWeightDimensionMismatch {
        index: usize,
        expected_width: usize,
        expected_height: usize,
        actual_width: usize,
        actual_height: usize,
    },

    #[error(
        "pixel weight {pixel_index} for drizzle frame {frame_index} must be finite and non-negative, got {value}"
    )]
    InvalidPixelWeight {
        frame_index: usize,
        pixel_index: usize,
        value: f32,
    },
}

#[cfg(test)]
mod tests {
    use crate::io::image::image_dimensions::ImageDimensions;
    use crate::stacking::drizzle::error::*;

    /// The two messages that name drizzle rather than stacking, which is what keeps this error a
    /// type of its own — and the shared payloads it reports through, whose wording lives with them.
    #[test]
    fn drizzle_names_itself_and_defers_for_the_shared_payloads() {
        assert_eq!(
            DrizzleError::NoFrames.to_string(),
            "No frames provided for drizzle"
        );
        assert_eq!(DrizzleError::Cancelled.to_string(), "drizzle cancelled");

        let mismatch = FrameDimensionMismatch::check(
            2,
            ImageDimensions::new((100, 100), 3),
            ImageDimensions::new((200, 100), 3),
        )
        .unwrap_err();
        assert_eq!(
            DrizzleError::from(mismatch).to_string(),
            "frame 2 is 200x100x3, expected 100x100x3"
        );
    }
}
