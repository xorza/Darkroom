//! Why a dialog's pick could not become an edit, why an image of the window went nowhere, and
//! why a driver's open left the editor as it was.

use std::path::PathBuf;

use image::ImageError;
use palantir::ClipboardUnavailable;

/// A picked path that is not valid Unicode. A node's path input holds a
/// `String`, and a lossy conversion would name a file that does not exist.
#[derive(Debug, thiserror::Error)]
#[error("{} is not valid Unicode, so a node input cannot hold it", .0.display())]
pub(super) struct NonUnicodePath(pub(super) PathBuf);

/// An image of the window that could not reach its target.
#[derive(Debug, thiserror::Error)]
pub(crate) enum ViewCaptureError {
    #[error("the window showed nothing to copy or save")]
    Empty,
    #[error("copying the view failed: {0}")]
    Clipboard(#[from] ClipboardUnavailable),
    #[error("saving the view to {} failed: {source}", .path.display())]
    Save {
        path: PathBuf,
        #[source]
        source: ImageError,
    },
}

/// Why a driver's open, or its new document, left the editor as it was.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum DrivenOpenError {
    /// The open document, at `document` or unsaved, has edits the open would discard.
    Unsaved { document: Option<PathBuf> },
    /// The file at `path` did not load; `status` is the editor's status line after it, which
    /// names why.
    NotOpened {
        path: PathBuf,
        status: Option<String>,
    },
}
