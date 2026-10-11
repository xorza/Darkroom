//! Unix: a rename replaces in one step, a directory is synced for its renames to last, and the
//! metadata states the device, the inode and the change time.

use std::fs::{self, File, Metadata};
use std::io;
use std::os::unix::fs::MetadataExt;
use std::path::Path;

use crate::file_utils::PublicationMode;
use crate::file_utils::file_identity::FileId;
use crate::file_utils::platform::{Observed, Stated};

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
