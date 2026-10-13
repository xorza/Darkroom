//! What Linux and macOS share: a scratch file unlinked while open, read-ahead advice, and the free
//! space of a file system.

use std::fs::{self, File, OpenOptions};
use std::io;
use std::os::unix::fs::MetadataExt;
use std::path::Path;

use memmap2::{Advice, Mmap};
use sysinfo::{Disk, Disks};

/// A new file at `path`, unlinked as soon as it is created, before any data goes in. A crash in
/// the instant between can leave it behind, empty.
pub(super) fn create_deleted(path: &Path) -> io::Result<File> {
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .open(path)?;
    fs::remove_file(path)?;
    Ok(file)
}

/// No: the map keeps an unlinked file alive by itself.
pub(super) const SCRATCH_MAP_NEEDS_HANDLE: bool = false;

/// The map is read front to back. A hint: a kernel that refuses it reads the same bytes, so its
/// refusal is not an error.
pub(super) fn advise_sequential(map: &Mmap) {
    map.advise(Advice::Sequential).ok();
}

/// The free space of the listed disk whose mount point is on the device `path` is on. The device,
/// not the path, ties the two: a path under a mount the list omits — a network share, a tmpfs —
/// would otherwise match the disk mounted above it, and a macOS firmlink would match none.
pub(super) fn free_space(path: &Path) -> Option<u64> {
    let device = fs::metadata(path).ok()?.dev();
    Disks::new_with_refreshed_list()
        .list()
        .iter()
        .find(|disk| fs::metadata(disk.mount_point()).is_ok_and(|mount| mount.dev() == device))
        .map(Disk::available_space)
}

#[cfg(feature = "internals")]
pub(crate) mod internals {
    /// No: a scratch file is unlinked as soon as it is created.
    #[cfg(test)]
    pub(crate) const SCRATCH_KEEPS_NAME: bool = false;
}
