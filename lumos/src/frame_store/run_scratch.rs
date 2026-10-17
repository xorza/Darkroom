//! [`RunScratch`]: where one run parks its frames, in files deleted while they are open.

use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::{io, process};

use bytemuck::Pod;

use crate::frame_store::disk_root::DiskRoot;
use crate::frame_store::error::FrameStoreError;
use crate::frame_store::stored_plane::StoredPlane;
use crate::platform::{Eviction, ScratchFile};

/// The files one run parks frames in, private to it and gone when it ends, however it ends: each
/// a [`ScratchFile`], deleted while it is open, so a frame's disk space returns the moment its
/// planes drop.
#[derive(Debug)]
pub(crate) struct RunScratch {
    root: DiskRoot,
    /// The next file's number. Two runs over one root count alike and pass over each other's
    /// names, which the exclusive create finds taken.
    next: AtomicU64,
    /// Whether a bench of cold reads may evict each map's pages; a run's scratch never does, so it
    /// holds no handle per plane.
    eviction: Eviction,
}

impl RunScratch {
    /// Scratch under `root`, created if missing.
    pub(crate) fn create(root: &Path) -> Result<Self, FrameStoreError> {
        Ok(Self {
            root: DiskRoot::create(root)?,
            next: AtomicU64::new(0),
            eviction: Eviction::Never,
        })
    }

    /// See [`DiskRoot::ensure_space`].
    pub(crate) fn ensure_space(&self, needed: u64) -> Result<(), FrameStoreError> {
        self.root.ensure_space(needed)
    }

    /// `pixels` in a file of their own, deleted while open, memory-mapped back.
    pub(crate) fn store<T: Pod>(&self, pixels: &[T]) -> Result<StoredPlane<T>, FrameStoreError> {
        let mut file = self.create_file()?;
        let write_error = |source| FrameStoreError::WriteFile {
            path: self.root.path().to_path_buf(),
            source,
        };
        file.write_all(bytemuck::cast_slice(pixels))
            .map_err(write_error)?;
        let mapped = file
            .map(self.eviction)
            .map_err(|source| FrameStoreError::MemoryMap {
                path: self.root.path().to_path_buf(),
                source,
            })?;
        Ok(StoredPlane::mapped(mapped))
    }

    /// A new file in the root that only this handle reaches: `.lumos-scratch-<pid>-<n>`, created
    /// exclusively so a name another holds is passed over.
    fn create_file(&self) -> Result<ScratchFile, FrameStoreError> {
        loop {
            let id = self.next.fetch_add(1, Ordering::Relaxed);
            let path = self
                .root
                .path()
                .join(format!(".lumos-scratch-{}-{id}", process::id()));
            match ScratchFile::create_new(&path) {
                Ok(file) => return Ok(file),
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
                Err(source) => return Err(FrameStoreError::OpenFile { path, source }),
            }
        }
    }
}

#[cfg(feature = "internals")]
pub(crate) mod internals {
    use std::path::Path;

    use crate::frame_store::error::FrameStoreError;
    use crate::frame_store::run_scratch::RunScratch;
    use crate::platform::Eviction;

    impl RunScratch {
        /// [`RunScratch::create`] whose planes a bench of cold reads can evict.
        pub(crate) fn create_evictable(root: &Path) -> Result<Self, FrameStoreError> {
            Ok(Self {
                eviction: Eviction::Allowed,
                ..Self::create(root)?
            })
        }
    }
}
