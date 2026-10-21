//! Why a dialog's pick could not become an edit.

use std::path::PathBuf;

/// A picked path that is not valid Unicode. A node's path input holds a
/// `String`, and a lossy conversion would name a file that does not exist.
#[derive(Debug, thiserror::Error)]
#[error("{} is not valid Unicode, so a node input cannot hold it", .0.display())]
pub(super) struct NonUnicodePath(pub(super) PathBuf);
