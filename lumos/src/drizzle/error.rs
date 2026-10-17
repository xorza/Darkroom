//! Error types for drizzle stacking.

use thiserror::Error;

use crate::combine::error::StackError;
use crate::error::InvalidConfigField;
use crate::frame_store::error::{FrameStoreError, LoadError};
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

/// Errors that can occur during drizzle stacking: the drizzle's own configuration, or a failure of
/// the run it shares with the combine — no frames, a cancel, a decode, a frame refused at
/// admission, a frame store — each with the one spelling [`StackError`] gives it.
#[derive(Debug, Error)]
pub enum DrizzleError {
    #[error(transparent)]
    Config(#[from] DrizzleConfigError),

    #[error(transparent)]
    Stack(#[from] StackError),
}

impl DrizzleError {
    /// Whether the run's cancel token stopped it, in whichever phase.
    pub const fn is_cancelled(&self) -> bool {
        matches!(self, Self::Stack(StackError::Cancelled))
    }
}

impl From<ImageError> for DrizzleError {
    fn from(error: ImageError) -> Self {
        Self::Stack(StackError::from(error))
    }
}

impl From<AdmissionError> for DrizzleError {
    fn from(error: AdmissionError) -> Self {
        Self::Stack(StackError::from(error))
    }
}

impl From<LoadError> for DrizzleError {
    fn from(error: LoadError) -> Self {
        Self::Stack(StackError::from(error))
    }
}

impl From<FrameStoreError> for DrizzleError {
    fn from(error: FrameStoreError) -> Self {
        Self::Stack(StackError::from(error))
    }
}

#[cfg(test)]
mod tests {
    use std::io;
    use std::path::{Path, PathBuf};

    use crate::drizzle::error::*;
    use crate::image::channel_layout::ChannelLayout;
    use crate::image::error::FrameDimensionMismatch;
    use crate::image::image_dimensions::ImageDimensions;

    /// The shared failures read the same from a drizzle as from a stack, their wording kept with
    /// the payload that owns it.
    #[test]
    fn shared_failures_report_through_the_stack_error() {
        assert_eq!(
            DrizzleError::from(StackError::NoFrames).to_string(),
            "no frames provided"
        );
        let mismatch = FrameDimensionMismatch::check(
            2,
            ImageDimensions::new((100, 100), ChannelLayout::Rgb),
            ImageDimensions::new((200, 100), ChannelLayout::Rgb),
        )
        .unwrap_err();
        assert_eq!(
            DrizzleError::from(AdmissionError::from(mismatch)).to_string(),
            "frame 2 is 200x100x3, expected 100x100x3"
        );
    }

    /// A cancel is one case whichever phase met it — a decode, an admission check, the combine —
    /// and any other decode failure is the file's, its error kept whole.
    #[test]
    fn a_cancel_is_one_case_from_every_phase() {
        let cancels = [
            DrizzleError::from(ImageError::cancelled(Path::new("light.fits"))),
            DrizzleError::from(AdmissionError::Cancelled),
            DrizzleError::from(StackError::Cancelled),
        ];
        for cancel in cancels {
            assert!(cancel.is_cancelled(), "{cancel:?}");
        }
        let missing = ImageError::Io {
            path: PathBuf::from("light.fits"),
            source: io::Error::from(io::ErrorKind::NotFound),
        };
        assert!(matches!(
            DrizzleError::from(missing),
            DrizzleError::Stack(StackError::ImageLoad(ImageError::Io { path, .. }))
                if path == Path::new("light.fits")
        ));
    }
}
