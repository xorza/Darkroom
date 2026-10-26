//! What Linux and macOS share: a scratch file unlinked while open, read-ahead advice, the free
//! space of a file system; a rename that replaces in one step, a directory synced for its renames
//! to last, and metadata that states the device, the inode and the change time.

use std::fs::{self, File, Metadata, OpenOptions};
use std::io;
use std::os::unix::fs::MetadataExt;
use std::path::Path;

use memmap2::{Advice, Mmap};
use sysinfo::{Disk, Disks};

use crate::fs::PublicationMode;
use crate::fs::file_identity::FileId;
use crate::platform::{Observed, Stated};

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

pub(super) fn symlink(original: &Path, link: &Path) -> io::Result<()> {
    std::os::unix::fs::symlink(original, link)
}

pub(super) fn replace(source: &Path, destination: &Path, _mode: PublicationMode) -> io::Result<()> {
    fs::rename(source, destination)
}

pub(super) fn sync_parent(parent: &Path) -> io::Result<()> {
    File::open(parent)?.sync_all()
}

/// The device and inode, and `st_ctime` at its full resolution.
pub(super) fn stated(metadata: &Metadata) -> Stated {
    Stated {
        file_id: Some(FileId {
            volume: metadata.dev(),
            file: u128::from(metadata.ino()),
        }),
        changed_ns: Some(
            i128::from(metadata.ctime()) * 1_000_000_000 + i128::from(metadata.ctime_nsec()),
        ),
    }
}

pub(super) fn observe(path: &Path) -> io::Result<Observed> {
    let metadata = fs::metadata(path)?;
    let stated = stated(&metadata);
    Ok(Observed { metadata, stated })
}

#[cfg(feature = "internals")]
pub(crate) mod internals {
    use std::fs::{self, Permissions};
    use std::os::unix::fs::PermissionsExt as _;
    use std::path::Path;

    /// No: a scratch file is unlinked as soon as it is created.
    pub(crate) const SCRATCH_KEEPS_NAME: bool = false;

    /// Mode `000` on `path`, and the permissions it had.
    #[expect(
        clippy::unnecessary_wraps,
        reason = "one signature on every system: Windows has no mode to take away"
    )]
    pub(crate) fn take_permissions(path: &Path) -> Option<Permissions> {
        let original = fs::metadata(path)
            .expect("an unreadable fixture starts from an existing path")
            .permissions();
        fs::set_permissions(path, Permissions::from_mode(0o000))
            .expect("take a fixture's permissions away");
        Some(original)
    }
}
