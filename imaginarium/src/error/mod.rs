use std::path::PathBuf;
use std::{io, result};

use common::Size2us;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    #[error("IO error: {0}")]
    Io(#[from] io::Error),
    #[error("no image file format has the extension of '{}'", .0.display())]
    InvalidExtension(PathBuf),
    #[error("Unsupported color type: {0}")]
    UnsupportedColorType(String),
    #[error("Unsupported format: {0}")]
    UnsupportedFormat(String),
    #[error("Invalid color format: {0}")]
    InvalidColorFormat(String),
    #[error("Size mismatch: {0}")]
    SizeMismatch(String),
    #[error("Image codec error: {0}")]
    ImageCodec(#[from] image::ImageError),
    #[error("TIFF codec error: {0}")]
    TiffCodec(#[from] tiff::TiffError),
    #[cfg(feature = "wgpu")]
    #[error("GPU error: {0}")]
    Gpu(String),
}

pub type Result<T> = result::Result<T, Error>;

/// Why a size and a channel layout describe no planar image.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum DescError {
    #[error("width must be positive")]
    ZeroWidth,
    #[error("height must be positive")]
    ZeroHeight,
    #[error("only 1 (mono) or 3 (RGB) planes supported, got {count}")]
    Channels { count: usize },
    #[error("{size}x{channel_count} samples overflow usize")]
    SampleCount { size: Size2us, channel_count: usize },
}

#[cfg(test)]
mod tests;
