//! [`FileIdentity`]: what a cache keyed on a file compares to tell whether the file changed.

use std::fs::{self, Metadata};
use std::io;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

/// A file's length and modification time: the identity every cache that is keyed on a source file
/// compares, instead of the file's contents.
///
/// It is as fine as the filesystem's timestamps and no finer. On a filesystem with coarse
/// timestamps (FAT keeps 2 s) an edit that keeps the length and lands within one tick of the
/// previous write keeps the identity too.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct FileIdentity {
    pub len: u64,
    /// Nanoseconds from the Unix epoch, **negative before it**. Signed because an unsigned offset
    /// gives every pre-1970 time the same `0` as the epoch itself.
    pub mtime_ns: i128,
}

impl FileIdentity {
    /// The identity of the file `path` names, following a symlink to its target.
    pub fn of(path: &Path) -> io::Result<Self> {
        Self::from_metadata(&fs::metadata(path)?)
    }

    /// Fails when the filesystem reports no modification time. Length alone is not an identity — a
    /// same-length edit would keep it — so the file is refused rather than half-identified.
    pub fn from_metadata(metadata: &Metadata) -> io::Result<Self> {
        Ok(Self {
            len: metadata.len(),
            mtime_ns: epoch_offset_ns(metadata.modified()?),
        })
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
mod tests {
    use std::fs;
    use std::time::{Duration, UNIX_EPOCH};

    use crate::TempDir;
    use crate::file_utils::file_identity::{FileIdentity, epoch_offset_ns};

    /// Hand-computed against the epoch: signed, so the two sides of it are ordered rather than
    /// folded together, at full nanosecond resolution on both.
    #[test]
    fn epoch_offset_is_signed_nanoseconds() {
        for (time, expected) in [
            (UNIX_EPOCH, 0),
            (UNIX_EPOCH + Duration::from_secs(1), 1_000_000_000),
            (UNIX_EPOCH - Duration::from_secs(1), -1_000_000_000),
            (UNIX_EPOCH - Duration::from_nanos(3), -3),
        ] {
            assert_eq!(epoch_offset_ns(time), expected, "{time:?}");
        }
    }

    /// The identity of a real file is its length and its mtime, and an append moves the length.
    #[test]
    fn identity_reads_length_and_mtime() {
        let directory = TempDir::new("file_identity");
        let path = directory.path().join("frame.fits");
        fs::write(&path, b"1234").unwrap();
        let identity = FileIdentity::of(&path).unwrap();
        let modified = fs::metadata(&path).unwrap().modified().unwrap();
        assert_eq!(
            identity,
            FileIdentity {
                len: 4,
                mtime_ns: epoch_offset_ns(modified),
            }
        );

        fs::write(&path, b"12345").unwrap();
        assert_eq!(FileIdentity::of(&path).unwrap().len, 5);
        assert!(FileIdentity::of(&directory.path().join("missing")).is_err());
    }
}
