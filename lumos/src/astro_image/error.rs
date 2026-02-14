use std::path::PathBuf;

use thiserror::Error;

/// Errors that can occur when loading an astronomical image from disk.
#[derive(Debug, Error)]
pub enum ImageLoadError {
    #[error("Failed to load FITS file '{path}': {source}")]
    Fits {
        path: PathBuf,
        source: fitsio::errors::Error,
    },

    #[error("Failed to load image '{path}': {source}")]
    Image {
        path: PathBuf,
        source: imaginarium::Error,
    },

    #[error("Failed to load raw file '{path}': {reason}")]
    Raw { path: PathBuf, reason: String },

    #[error("Failed to read file '{path}': {source}")]
    Io {
        path: PathBuf,
        source: std::io::Error,
    },

    #[error("Unsupported file extension: '{extension}'")]
    UnsupportedFormat { extension: String },
}
