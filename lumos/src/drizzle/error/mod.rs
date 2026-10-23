//! Error types for drizzle stacking.

use thiserror::Error;

use crate::combine::error::StackError;
use crate::error::InvalidConfigField;
use crate::frame_store::error::{FrameStoreError, LoadError};
use crate::image::error::DimensionsError;
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

    /// The output grid the scale makes of the input frames is no image's.
    #[error("drizzle output grid: {0}")]
    OutputSize(#[from] DimensionsError),
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
mod tests;
