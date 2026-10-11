//! [`CachedSource`]: a source file as the decode cache names it.

use std::fs;
use std::path::{Path, PathBuf};

use common::FileIdentity;

use crate::frame_store::error::FrameStoreError;

/// A source file as the decode cache names it: its canonical path, and its identity when it was
/// read.
#[derive(Debug)]
pub(crate) struct CachedSource {
    pub(crate) canonical: PathBuf,
    pub(crate) identity: FileIdentity,
}

impl CachedSource {
    /// `path` as it is now.
    pub(crate) fn of(path: &Path) -> Result<Self, FrameStoreError> {
        let metadata_error = |source| FrameStoreError::ReadMetadata {
            path: path.to_path_buf(),
            source,
        };
        let canonical = fs::canonicalize(path).map_err(metadata_error)?;
        let identity = FileIdentity::of(&canonical).map_err(metadata_error)?;
        Ok(Self {
            canonical,
            identity,
        })
    }
}
