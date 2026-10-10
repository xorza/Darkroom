//! [`RunScratch`]: where one run parks its frames, in files deleted while they are open.

use std::fs::{File, OpenOptions};
use std::io::{self, Write};
use std::path::Path;
use std::process;
use std::sync::atomic::{AtomicU64, Ordering};

use bytemuck::Pod;

use crate::frame_store::disk_root::DiskRoot;
use crate::frame_store::error::FrameStoreError;
use crate::frame_store::frame_quality::FramePlane;
use crate::frame_store::plane_store::PlaneStore;
use crate::frame_store::stored_plane::StoredPlane;
use crate::mount_table::MountTable;

/// The files one run parks frames in, private to it and gone when it ends, however it ends.
///
/// Each file is deleted while it is open: on Unix it is unlinked as soon as it is created, before
/// any data goes in, and lives on in its memory map; on Windows it is opened to be deleted when its
/// last handle closes, and the map holds the handle. Nothing is left for a later run to clean up
/// after a crash, no other run can open a file that has no name, and a frame's disk space returns
/// the moment its planes drop. On Unix a crash in the instant between a file's creation and its
/// unlink can leave it behind, empty.
#[derive(Debug)]
pub(crate) struct RunScratch {
    root: DiskRoot,
}

impl RunScratch {
    /// Scratch under `root`, created if missing.
    pub(crate) fn create(root: &Path) -> Result<Self, FrameStoreError> {
        Ok(Self {
            root: DiskRoot::create(root, &MountTable::read())?,
        })
    }

    /// `pixels` in a file of their own, deleted while open, memory-mapped back.
    pub(crate) fn store<T: Pod>(&self, pixels: &[T]) -> Result<StoredPlane<T>, FrameStoreError> {
        let mut file = self.create_file()?;
        file.write_all(bytemuck::cast_slice(pixels))
            .map_err(|source| FrameStoreError::WriteFile {
                path: self.root.path().to_path_buf(),
                source,
            })?;
        StoredPlane::map_open(file, self.root.path())
    }

    /// A new file in the root that only this handle reaches: `.lumos-scratch-<pid>-<n>`, created
    /// exclusively so a name another process holds is passed over, then deleted.
    fn create_file(&self) -> Result<File, FrameStoreError> {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        loop {
            let id = COUNTER.fetch_add(1, Ordering::Relaxed);
            let path = self
                .root
                .path()
                .join(format!(".lumos-scratch-{}-{id}", process::id()));
            match Self::open_deleting(&path) {
                Ok(file) => return Ok(file),
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
                Err(source) => return Err(FrameStoreError::OpenFile { path, source }),
            }
        }
    }

    #[cfg(unix)]
    fn open_deleting(path: &Path) -> io::Result<File> {
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .open(path)?;
        std::fs::remove_file(path)?;
        Ok(file)
    }

    /// `FILE_FLAG_DELETE_ON_CLOSE`, with every share mode so the map can open the file beside the
    /// handle; the values are the Win32 headers'.
    #[cfg(windows)]
    fn open_deleting(path: &Path) -> io::Result<File> {
        use std::os::windows::fs::OpenOptionsExt;
        const FILE_FLAG_DELETE_ON_CLOSE: u32 = 0x0400_0000;
        const FILE_SHARE_READ_WRITE_DELETE: u32 = 0x1 | 0x2 | 0x4;
        OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .custom_flags(FILE_FLAG_DELETE_ON_CLOSE)
            .share_mode(FILE_SHARE_READ_WRITE_DELETE)
            .open(path)
    }
}

impl PlaneStore for RunScratch {
    fn store_channel(
        &self,
        _channel: usize,
        pixels: &[f32],
    ) -> Result<StoredPlane, FrameStoreError> {
        self.store(pixels)
    }

    fn store_quality(
        &self,
        _plane: FramePlane,
        pixels: &[f32],
    ) -> Result<StoredPlane, FrameStoreError> {
        self.store(pixels)
    }

    fn store_flags(&self, bytes: &[u8]) -> Result<StoredPlane<u8>, FrameStoreError> {
        self.store(bytes)
    }

    fn store_samples(&self, _slot: usize, samples: &[f32]) -> Result<StoredPlane, FrameStoreError> {
        self.store(samples)
    }

    fn store_gain(&self, _channel: usize, nodes: &[f32]) -> Result<StoredPlane, FrameStoreError> {
        self.store(nodes)
    }
}
