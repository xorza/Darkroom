//! [`DiskRoot`]: a directory the frame store writes into, held to disk.

use std::fs;
use std::path::{Path, PathBuf};

use crate::frame_store::error::FrameStoreError;
use crate::platform::FileSystem;

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
    /// `path`, created with its parents, refused on a memory-backed file system.
    pub(crate) fn create(path: &Path) -> Result<Self, FrameStoreError> {
        fs::create_dir_all(path).map_err(|source| FrameStoreError::CreateDirectory {
            path: path.to_path_buf(),
            source,
        })?;
        let resolved =
            fs::canonicalize(path).map_err(|source| FrameStoreError::CreateDirectory {
                path: path.to_path_buf(),
                source,
            })?;
        if let Some(filesystem) = FileSystem::memory_backed(&resolved) {
            return Err(FrameStoreError::MemoryBackedDirectory {
                path: path.to_path_buf(),
                filesystem,
            });
        }
        Ok(Self { path: resolved })
    }

    pub(crate) fn path(&self) -> &Path {
        &self.path
    }

    /// Whether the disk holding the directory has `needed` bytes free: refused before a run
    /// writes, rather than when the disk fills hours into it. Passes when the disk could not be
    /// read.
    ///
    /// # Errors
    ///
    /// [`FrameStoreError::InsufficientSpace`].
    pub(crate) fn ensure_space(&self, needed: u64) -> Result<(), FrameStoreError> {
        space_for(&self.path, needed, FileSystem::free_space(&self.path))
    }
}

/// `needed` bytes against the `available` the disk holding `path` has free.
fn space_for(path: &Path, needed: u64, available: Option<u64>) -> Result<(), FrameStoreError> {
    match available {
        Some(available) if available < needed => Err(FrameStoreError::InsufficientSpace {
            path: path.to_path_buf(),
            needed,
            available,
        }),
        _ => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use crate::frame_store::disk_root::space_for;
    use crate::frame_store::error::FrameStoreError;

    /// A spill that needs more than the disk holds is refused with both figures; one that fits, or
    /// a disk that could not be read, passes.
    #[test]
    fn a_spill_the_disk_cannot_hold_is_refused() {
        let path = Path::new("/scratch");
        let error = space_for(path, 2_100_000, Some(2_099_999)).unwrap_err();
        assert!(
            matches!(
                &error,
                FrameStoreError::InsufficientSpace { path: at, needed: 2_100_000, available: 2_099_999 }
                    if at == path
            ),
            "{error:?}"
        );
        assert!(space_for(path, 2_100_000, Some(2_100_000)).is_ok());
        assert!(space_for(path, 2_100_000, None).is_ok());
    }
}
