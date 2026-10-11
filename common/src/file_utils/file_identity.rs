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
mod tests {
    use std::fs;
    use std::time::{Duration, UNIX_EPOCH};

    use crate::TempDir;
    use crate::file_utils::file_identity::{FileId, FileIdentity, epoch_offset_ns};

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
            (identity.len, identity.mtime_ns),
            (4, epoch_offset_ns(modified))
        );

        fs::write(&path, b"12345").unwrap();
        assert_eq!(FileIdentity::of(&path).unwrap().len, 5);
        assert!(FileIdentity::of(&directory.path().join("missing")).is_err());
    }

    /// The layout a digest folds, byte by byte: the length and the mtime, then a `1` and the id's
    /// volume and file, then a `0` for a change time not stated — 8 + 16 + 1 + 8 + 16 + 1 = 50
    /// bytes. A stated change time is a `1` and its 16 bytes instead.
    #[test]
    fn write_to_folds_every_field_in_a_fixed_layout() {
        let mut identity = FileIdentity {
            len: 4,
            mtime_ns: -3,
            file_id: Some(FileId { volume: 2, file: 5 }),
            changed_ns: None,
        };
        let folded = |identity: &FileIdentity| {
            let mut bytes = Vec::new();
            identity.write_to(|part| bytes.extend_from_slice(part));
            bytes
        };
        let mut expected = Vec::new();
        expected.extend_from_slice(&4u64.to_le_bytes());
        expected.extend_from_slice(&(-3i128).to_le_bytes());
        expected.push(1);
        expected.extend_from_slice(&2u64.to_le_bytes());
        expected.extend_from_slice(&5u128.to_le_bytes());
        expected.push(0);
        assert_eq!(folded(&identity), expected);

        identity.changed_ns = Some(7);
        expected.pop();
        expected.push(1);
        expected.extend_from_slice(&7i128.to_le_bytes());
        assert_eq!(folded(&identity), expected);
    }

    /// A rewrite that keeps the length and sets the mtime back keeps both, and moves the change
    /// time, which no call sets; a `chmod` moves it as well, and a file renamed into place moves
    /// the id. The kernel stamps the change time from a clock as coarse as a few milliseconds, so
    /// each step waits 20 ms past the last.
    #[cfg(unix)]
    #[test]
    fn a_rewrite_that_restores_the_mtime_moves_the_identity() {
        use std::fs::{File, Permissions};
        use std::os::unix::fs::PermissionsExt;
        use std::thread;

        let directory = TempDir::new("file_identity_rewrite");
        let path = directory.path().join("frame.fits");
        fs::write(&path, b"1234").unwrap();
        let original = FileIdentity::of(&path).unwrap();
        let modified = fs::metadata(&path).unwrap().modified().unwrap();
        assert!(original.file_id.is_some() && original.changed_ns.is_some());
        assert_eq!(
            FileIdentity::from_metadata(&fs::metadata(&path).unwrap()).unwrap(),
            original
        );
        let settle = || thread::sleep(Duration::from_millis(20));

        settle();
        fs::write(&path, b"5678").unwrap();
        File::options()
            .write(true)
            .open(&path)
            .unwrap()
            .set_modified(modified)
            .unwrap();
        let rewritten = FileIdentity::of(&path).unwrap();
        assert_eq!(
            (rewritten.len, rewritten.mtime_ns, rewritten.file_id),
            (original.len, original.mtime_ns, original.file_id)
        );
        assert_ne!(rewritten.changed_ns, original.changed_ns);

        settle();
        fs::set_permissions(&path, Permissions::from_mode(0o600)).unwrap();
        let chmodded = FileIdentity::of(&path).unwrap();
        assert_ne!(chmodded.changed_ns, rewritten.changed_ns);

        let replacement = directory.path().join("frame.tmp");
        fs::write(&replacement, b"5678").unwrap();
        fs::rename(&replacement, &path).unwrap();
        assert_ne!(FileIdentity::of(&path).unwrap().file_id, original.file_id);
    }
}
