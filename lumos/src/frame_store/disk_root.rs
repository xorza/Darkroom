//! [`DiskRoot`]: a directory the frame store writes into, held to disk.

use std::fs;
use std::path::{Path, PathBuf};

use crate::frame_store::error::FrameStoreError;
use crate::mount_table::MountTable;

/// A directory the frame store writes into, created if it is missing, and on a file system that
/// keeps its files on disk.
///
/// A frame spills because the run's frames do not fit in memory. On tmpfs or ramfs — `/tmp` on
/// Debian 13 and Arch — every spilled byte is memory again, so the spill fills the file system and
/// evicts the pages the plan counted on. Such a root is refused rather than written into.
#[derive(Debug)]
pub(crate) struct DiskRoot {
    path: PathBuf,
}

impl DiskRoot {
    /// `path`, created with its parents, checked against `mounts`.
    pub(crate) fn create(path: &Path, mounts: &MountTable) -> Result<Self, FrameStoreError> {
        fs::create_dir_all(path).map_err(|source| FrameStoreError::CreateDirectory {
            path: path.to_path_buf(),
            source,
        })?;
        let resolved =
            fs::canonicalize(path).map_err(|source| FrameStoreError::CreateDirectory {
                path: path.to_path_buf(),
                source,
            })?;
        if let Some(mount) = mounts
            .mount_of(&resolved)
            .filter(|mount| mount.is_memory_backed())
        {
            return Err(FrameStoreError::MemoryBackedDirectory {
                path: path.to_path_buf(),
                filesystem: mount.filesystem.clone(),
            });
        }
        Ok(Self { path: resolved })
    }

    pub(crate) fn path(&self) -> &Path {
        &self.path
    }
}
