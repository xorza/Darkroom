//! Error types for drizzle stacking.

use thiserror::Error;

use crate::combine::error::StackError;
use crate::error::InvalidConfigField;
use crate::frame_store::error::{FrameLoadError, FrameStoreError};
use crate::image::error::FrameDescError;
use crate::ingest::error::AdmissionError;
use crate::io::error::ImageError;

/// Errors that can occur during drizzle stacking: the drizzle's own configuration, or a failure of
/// the run it shares with the combine — no frames, a cancel, a decode, a frame refused at
/// admission, a frame store — each with the one spelling [`StackError`] gives it.
#[derive(Debug, Error)]
pub enum DrizzleError {
    /// A [`DrizzleConfig`](crate::DrizzleConfig) field out of its range.
    #[error("invalid drizzle configuration")]
    Config(#[from] InvalidConfigField),

    #[error(transparent)]
    Stack(#[from] StackError),

    /// The output grid the scale makes of the input frames is no image's.
    #[error("drizzle output grid")]
    OutputSize(#[from] FrameDescError),
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

impl From<FrameLoadError> for DrizzleError {
    fn from(error: FrameLoadError) -> Self {
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
