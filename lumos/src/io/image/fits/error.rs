use std::io::{Error as IoError, ErrorKind};
use std::path::Path;

use crate::io::image::error::ImageError;

pub(crate) fn fits_err(path: &Path, source: fits_well::FitsError) -> ImageError {
    ImageError::Fits {
        path: path.to_path_buf(),
        source,
    }
}

pub(crate) fn fits_unsupported(path: &Path, reason: impl Into<String>) -> ImageError {
    ImageError::FitsUnsupported {
        path: path.to_path_buf(),
        reason: reason.into(),
    }
}

pub(crate) fn fits_to_io(source: fits_well::FitsError) -> IoError {
    match source {
        fits_well::FitsError::Io(source) => source,
        source => IoError::new(ErrorKind::InvalidData, source),
    }
}
