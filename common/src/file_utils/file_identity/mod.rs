//! [`FileIdentity`]: what a cache keyed on a file compares to tell whether the file changed.

use std::fs::Metadata;
use std::io;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::file_utils::platform;
use crate::file_utils::platform::Stated;

/// A file's length and modification time, and where the system states them its id and its change
/// time: the identity every cache that is keyed on a source file compares, instead of the file's
/// contents.
///
/// Length and modification time alone miss a rewrite that keeps both: `cp -p`, `rsync -t` and a
/// re-export set the modification time back. The change time is the system's own, and no tool sets
/// it; a rewrite through another file renamed into place moves the id. Either also moves on a
/// change of the metadata alone — a `chmod`, a new link — which a cache then takes for a change of
/// the file: a rebuild, never a wrong read.
///
/// The times are as fine as the filesystem's and no finer. On a filesystem with coarse
/// timestamps (FAT keeps 2 s) an edit that keeps the length and lands within one tick of the
/// previous write keeps the identity too.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct FileIdentity {
    pub len: u64,
    /// Nanoseconds from the Unix epoch, **negative before it**. Signed because an unsigned offset
    /// gives every pre-1970 time the same `0` as the epoch itself.
    pub mtime_ns: i128,
    /// The file's id on its volume; `None` where the system, or the call that read the identity,
    /// states none — see [`Self::from_metadata`].
    pub file_id: Option<FileId>,
    /// When the file or its metadata last changed — `st_ctime` on Unix, the change time on
    /// Windows — in nanoseconds from the Unix epoch; `None` where it is not stated.
    pub changed_ns: Option<i128>,
}

/// A file's id on its system: the device and inode on Unix, the volume serial number and the
/// 128-bit file id on Windows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct FileId {
    pub volume: u64,
    pub file: u128,
}

impl FileIdentity {
    /// The identity of the file `path` names, following a symlink to its target, with everything
    /// the system states of it.
    pub fn of(path: &Path) -> io::Result<Self> {
        let observed = platform::observe(path)?;
        Self::stated(&observed.metadata, observed.stated)
    }

    /// The identity `metadata` holds: on Unix all of it, on Windows, whose `Metadata` states no id
    /// or change time, the length and modification time alone. Two identities compared are to come
    /// from the same call.
    ///
    /// Fails when the filesystem reports no modification time. Length alone is not an identity — a
    /// same-length edit would keep it — so the file is refused rather than half-identified.
    pub fn from_metadata(metadata: &Metadata) -> io::Result<Self> {
        Self::stated(metadata, platform::stated(metadata))
    }

    fn stated(metadata: &Metadata, stated: Stated) -> io::Result<Self> {
        Ok(Self {
            len: metadata.len(),
            mtime_ns: epoch_offset_ns(metadata.modified()?),
            file_id: stated.file_id,
            changed_ns: stated.changed_ns,
        })
    }

    /// Every field in a fixed little-endian layout, a field the system did not state as a `0` byte
    /// and one it did as a `1` before its value: what a digest keyed on the identity folds.
    pub fn write_to(&self, mut write: impl FnMut(&[u8])) {
        write(&self.len.to_le_bytes());
        write(&self.mtime_ns.to_le_bytes());
        match self.file_id {
            Some(FileId { volume, file }) => {
                write(&[1]);
                write(&volume.to_le_bytes());
                write(&file.to_le_bytes());
            }
            None => write(&[0]),
        }
        match self.changed_ns {
            Some(changed) => {
                write(&[1]);
                write(&changed.to_le_bytes());
            }
            None => write(&[0]),
        }
    }
}

/// Signed nanoseconds between `time` and the Unix epoch.
///
/// Separate from [`FileIdentity::from_metadata`] because the pre-epoch arm is the reason it exists,
/// and a test cannot set a real file's mtime to 1969 without a syscall this crate has no
/// dependency for.
fn epoch_offset_ns(time: SystemTime) -> i128 {
    match time.duration_since(UNIX_EPOCH) {
        Ok(after) => i128::try_from(after.as_nanos()).expect("a time offset in ns fits i128"),
        Err(before) => {
            -i128::try_from(before.duration().as_nanos()).expect("a time offset in ns fits i128")
        }
    }
}

#[cfg(test)]
mod tests;
