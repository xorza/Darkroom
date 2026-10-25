//! [`RasterFileError`]: why an image file does not load or save.

use std::io;
use std::path::PathBuf;

use imaginarium::CodecError;

/// Why the image file at a path does not load or save.
#[derive(Debug, thiserror::Error)]
pub(super) enum RasterFileError {
    #[error("no image file format has the extension of '{}'", path.display())]
    Extension { path: PathBuf },
    #[error("I/O error: {0}")]
    Io(#[from] io::Error),
    #[error(transparent)]
    Codec(#[from] CodecError),
}
