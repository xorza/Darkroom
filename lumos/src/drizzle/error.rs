//! Error types for drizzle stacking.

use thiserror::Error;

use crate::combine::error::StackError;
use crate::error::InvalidConfigField;
use crate::ingest::error::AdmissionError;
use crate::io::error::ImageError;

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

    /// A file that could not be decoded — see [`StackError::ImageLoad`]
    /// for why the decoder's own error travels rather than a wrapper. A cancelled decode arrives as
    /// [`Self::Cancelled`], never here.
    #[error(transparent)]
    ImageLoad(ImageError),

    /// A frame failed the checks every frame of a set passes: among them a frame of another
    /// shape, a frame of another mosaic pattern, whose photosites would reach other channels at
    /// the same positions, and a sample no drop can carry, whose value would turn every output
    /// pixel the drop reaches, and its variance, into the same non-finite number.
    #[error(transparent)]
    Admission(AdmissionError),

    /// The combine of the drizzled frames failed: its configuration, or storing a frame.
    #[error(transparent)]
    Stack(#[from] StackError),

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

/// Cancellation is the run stopping, not a file failing, so a cancelled decode leaves the load
/// error behind and reports as the drizzle's own.
impl From<ImageError> for DrizzleError {
    fn from(error: ImageError) -> Self {
        match error {
            ImageError::Cancelled { .. } => Self::Cancelled,
            error => Self::ImageLoad(error),
        }
    }
}

impl From<AdmissionError> for DrizzleError {
    fn from(error: AdmissionError) -> Self {
        match error {
            AdmissionError::Cancelled => Self::Cancelled,
            error => Self::Admission(error),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::io;
    use std::path::{Path, PathBuf};

    use crate::drizzle::error::*;
    use crate::error::FrameDimensionMismatch;
    use crate::image::image_dimensions::ImageDimensions;

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
            DrizzleError::from(AdmissionError::from(mismatch)).to_string(),
            "frame 2 is 200x100x3, expected 100x100x3"
        );
    }

    /// A decode the run's cancel stopped is the drizzle's own cancellation, not a file that failed
    /// to load; any other decode failure is the file's, its error kept whole.
    #[test]
    fn a_cancelled_decode_is_the_drizzles_cancellation() {
        assert!(matches!(
            DrizzleError::from(ImageError::cancelled(Path::new("light.fits"))),
            DrizzleError::Cancelled
        ));
        let missing = ImageError::Io {
            path: PathBuf::from("light.fits"),
            source: io::Error::from(io::ErrorKind::NotFound),
        };
        assert!(matches!(
            DrizzleError::from(missing),
            DrizzleError::ImageLoad(ImageError::Io { path, .. }) if path == Path::new("light.fits")
        ));
    }
}
