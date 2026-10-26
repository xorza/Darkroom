use std::io;
use std::path::PathBuf;

use common::{TempDir, Unreadable, fs};

use crate::io::raw::error::RawScanError;
use crate::io::raw::raw_files::raw_files;

/// Every RAW extension in any case is kept, a non-RAW file and a directory named like a RAW
/// file are not, and the result is in path order.
#[test]
fn populated_directory_is_filtered_case_insensitively_and_sorted() {
    let dir = TempDir::new("lumos-raw-files");
    for name in [
        "z.raf",
        "a.RAF",
        "b.Cr3",
        "c.nef",
        "ignored.fit",
        "no_extension",
    ] {
        fs::write(dir.join(name), []).unwrap();
    }
    fs::create_dir(dir.join("nested.raf")).unwrap();

    let files = raw_files(dir.path()).unwrap();

    let names: Vec<&str> = files
        .iter()
        .map(|path| path.file_name().unwrap().to_str().unwrap())
        .collect();
    assert_eq!(names, ["a.RAF", "b.Cr3", "c.nef", "z.raf"]);
}

#[test]
fn readable_empty_directory_is_distinct_from_scan_failure() {
    let dir = TempDir::new("lumos-raw-files");
    assert_eq!(raw_files(dir.path()).unwrap(), Vec::<PathBuf>::new());
}

#[test]
fn scan_failures_name_the_path() {
    let dir = TempDir::new("lumos-raw-files");
    let file = dir.join("frame.raf");
    fs::write(&file, []).unwrap();
    let error = raw_files(&file).unwrap_err();
    assert!(
        matches!(&error, RawScanError::Dir { path, .. } if *path == file),
        "{error:?}"
    );
    assert!(error.to_string().contains(&file.display().to_string()));

    let missing = dir.join("missing");
    let error = raw_files(&missing).unwrap_err();
    assert!(
        matches!(&error, RawScanError::Dir { path, source }
            if *path == missing && source.kind() == io::ErrorKind::NotFound),
        "{error:?}"
    );
    assert!(error.to_string().contains(&missing.display().to_string()));
}

/// Only an entry named as a RAW file is examined: a dangling `notes.txt` link leaves the scan
/// whole, a link named `link.cr2` to a RAW file is kept, and a dangling `x.cr2` link is an error
/// that names it.
#[cfg(unix)]
#[test]
fn only_raw_named_links_are_followed() {
    let dir = TempDir::new("lumos-raw-files");
    fs::write(dir.join("a.cr2"), []).unwrap();
    fs::symlink(dir.join("gone.txt"), dir.join("notes.txt")).unwrap();
    fs::symlink(dir.join("a.cr2"), dir.join("link.cr2")).unwrap();
    assert_eq!(
        raw_files(dir.path()).unwrap(),
        [dir.join("a.cr2"), dir.join("link.cr2")]
    );

    let dangling = dir.join("x.cr2");
    fs::symlink(dir.join("gone.cr2"), &dangling).unwrap();
    let error = raw_files(dir.path()).unwrap_err();
    assert!(
        matches!(&error, RawScanError::File { path, source }
            if *path == dangling && source.kind() == io::ErrorKind::NotFound),
        "{error:?}"
    );
}

#[test]
fn unreadable_directory_returns_contextual_error() {
    let dir = TempDir::new("lumos-raw-files");
    let Some(unreadable) = Unreadable::new(dir.path()) else {
        return;
    };
    let result = raw_files(dir.path());
    drop(unreadable);

    let error = result.unwrap_err();
    assert!(
        matches!(&error, RawScanError::Dir { source, .. }
            if source.kind() == io::ErrorKind::PermissionDenied),
        "{error:?}"
    );
    assert!(
        error
            .to_string()
            .contains(&dir.path().display().to_string())
    );
}
