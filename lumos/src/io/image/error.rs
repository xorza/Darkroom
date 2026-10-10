use std::io;
use std::path::{Path, PathBuf};

use thiserror::Error;

use crate::io::raw::error::{LibrawCode, RawError};

/// Errors that can occur when loading an astronomical image from disk.
#[derive(Debug, Error)]
pub enum ImageError {
    #[error("Image load cancelled for '{path}'")]
    Cancelled { path: PathBuf },

    #[error("Failed to load FITS file '{path}': {source}")]
    Fits {
        path: PathBuf,
        source: fits_well::FitsError,
    },

    #[error("Unsupported FITS file '{path}': {reason}")]
    FitsUnsupported { path: PathBuf, reason: String },

    #[error("Failed to load image '{path}': {source}")]
    Image {
        path: PathBuf,
        source: imaginarium::Error,
    },

    #[error("Failed to load raw file '{path}': {source}")]
    Raw { path: PathBuf, source: RawError },

    #[error("Failed to read file '{path}': {source}")]
    Io { path: PathBuf, source: io::Error },

    #[error("No decoder reads the extension of '{path}'")]
    UnsupportedFormat { path: PathBuf },

    #[error("Scientific image input '{path}' was rejected: {reason}")]
    ScientificInputRejected { path: PathBuf, reason: String },

    #[error("Failed to save image: {source}")]
    Save { source: imaginarium::Error },
}

impl ImageError {
    /// LibRaw's or the decoder's `source` failure reading `path`: [`Self::Cancelled`] where LibRaw
    /// stopped because the load's cancel token asked it to.
    pub(crate) fn raw(path: &Path, source: RawError) -> Self {
        match source {
            RawError::Open(LibrawCode::Cancelled)
            | RawError::Unpack(LibrawCode::Cancelled)
            | RawError::Process(LibrawCode::Cancelled) => Self::cancelled(path),
            source => Self::Raw {
                path: path.to_path_buf(),
                source,
            },
        }
    }

    /// The load of `path` stopped by its cancel token.
    pub(crate) fn cancelled(path: &Path) -> Self {
        Self::Cancelled {
            path: path.to_path_buf(),
        }
    }

    /// fits-well's `source` failure reading `path`.
    pub(crate) fn fits(path: &Path, source: fits_well::FitsError) -> Self {
        Self::Fits {
            path: path.to_path_buf(),
            source,
        }
    }

    /// A FITS file at `path` this decoder refuses, for `reason`.
    pub(crate) fn fits_unsupported(path: &Path, reason: impl Into<String>) -> Self {
        Self::FitsUnsupported {
            path: path.to_path_buf(),
            reason: reason.into(),
        }
    }

    /// An image at `path` refused as scientific input, for `reason`.
    pub(crate) fn scientific_rejection(path: &Path, reason: impl Into<String>) -> Self {
        Self::ScientificInputRejected {
            path: path.to_path_buf(),
            reason: reason.into(),
        }
    }
}
