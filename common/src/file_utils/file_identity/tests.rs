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
