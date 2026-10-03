use std::fs;
use std::io;
use std::path::PathBuf;

use common::TempDir;

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
    assert!(error.to_string().contains(&file.display().to_string()));

    let missing = dir.join("missing");
    let error = raw_files(&missing).unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::NotFound);
    assert!(error.to_string().contains(&missing.display().to_string()));
}

#[cfg(unix)]
#[test]
fn unreadable_directory_returns_contextual_error() {
    use common::Unreadable;

    let dir = TempDir::new("lumos-raw-files");
    let Some(unreadable) = Unreadable::new(dir.path()) else {
        return;
    };
    let result = raw_files(dir.path());
    drop(unreadable);

    let error = result.unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
    assert!(
        error
            .to_string()
            .contains(&dir.path().display().to_string())
    );
}
