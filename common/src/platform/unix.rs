//! What Linux and macOS share: a scratch file unlinked while open, read-ahead advice, the free
//! space of a file system; a rename that replaces in one step, a directory synced for its renames
//! to last, metadata that states the device, the inode and the change time, and a program left
//! running that leaves no zombie.

use std::ffi::OsStr;
use std::fs::{self, File, Metadata, OpenOptions};
use std::io;
use std::os::unix::fs::MetadataExt;
use std::path::Path;
use std::process::Command;

use memmap2::{Advice, Mmap};
use sysinfo::{Disk, Disks};

use crate::fs::PublicationMode;
use crate::fs::file_identity::FileId;
use crate::platform::{Observed, Stated};
use crate::thread;

/// Start `program` with `args` and leave it running: nothing reads its output, and a thread of
/// its own waits for its exit, so the system reaps it rather than keep a zombie until this
/// process ends.
pub(super) fn spawn_detached(
    program: impl AsRef<OsStr>,
    args: impl IntoIterator<Item = impl AsRef<OsStr>>,
) -> io::Result<()> {
    let mut child = Command::new(program).args(args).spawn()?;
    thread::spawn("child-reaper", move || child.wait().ok());
    Ok(())
}

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
///
/// # Panics
///
/// On a target sysinfo has no implementation for: a build-configuration error, not a runtime
/// condition.
pub(super) fn free_space(path: &Path) -> Option<u64> {
    let device = fs::metadata(path).ok()?.dev();
    Disks::new_with_refreshed_list()
        .expect("sysinfo has no implementation for this platform")
        .iter()
        .find(|disk| fs::metadata(disk.mount_point()).is_ok_and(|mount| mount.dev() == device))
        .map(Disk::available_space)
}

pub(super) fn symlink(original: &Path, link: &Path) -> io::Result<()> {
    std::os::unix::fs::symlink(original, link)
}

/// A link to a folder is a link as any other on Unix.
pub(super) fn symlink_dir(original: &Path, link: &Path) -> io::Result<()> {
    symlink(original, link)
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
    use std::ffi::OsString;
    use std::fs::{self, Permissions};
    use std::os::unix::ffi::OsStringExt as _;
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

    /// Yes: a link needs no privilege.
    pub(crate) const fn symlinks_allowed(_dir: &Path) -> bool {
        true
    }

    /// One byte, `0x80 + idx`: no UTF-8 sequence holds one alone.
    pub(crate) fn non_unicode_name(idx: u8) -> OsString {
        OsString::from_vec(vec![0x80 + idx])
    }

    /// Yes: `Metadata` holds the device, the inode and the change time.
    #[cfg(test)]
    pub(crate) const METADATA_HOLDS_STATED: bool = true;

    /// Mode `0o640`, which a new file does not take under the usual umask, and the permissions
    /// `path` then has.
    #[cfg(test)]
    #[expect(
        clippy::unnecessary_wraps,
        reason = "one signature on every system: Windows has no such permissions"
    )]
    pub(crate) fn set_uncommon_permissions(path: &Path) -> Option<Permissions> {
        fs::set_permissions(path, Permissions::from_mode(0o640)).unwrap();
        Some(fs::metadata(path).unwrap().permissions())
    }
}
