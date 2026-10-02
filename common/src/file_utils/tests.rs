use std::fs;
use std::io;
use std::io::{Seek as _, SeekFrom, Write as _};
use std::path::PathBuf;
use std::sync::{Arc, Barrier};

use tokio::io::AsyncWriteExt as _;

use crate::TempDir;
use crate::file_utils::{
    AtomicFile, PublicationMode, SyncAtomicFile, files_with_extensions, publish, publish_bytes,
    publish_with_replacement, replace,
};
use std::path::Path;
use std::thread;

fn names(paths: &[PathBuf]) -> Vec<&str> {
    paths
        .iter()
        .map(|path| path.file_name().unwrap().to_str().unwrap())
        .collect()
}

fn publication_temp_files(path: &Path) -> Vec<PathBuf> {
    let parent = path.parent().unwrap();
    let prefix = format!("{}.", path.file_name().unwrap().to_string_lossy());
    fs::read_dir(parent)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|candidate| {
            candidate
                .file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with(&prefix)
                && candidate
                    .extension()
                    .is_some_and(|extension| extension == "tmp")
        })
        .collect()
}

#[test]
fn populated_directory_is_filtered_case_insensitively_and_sorted() {
    let dir = TempDir::new("common-file-utils");
    fs::write(dir.join("z.raf"), []).unwrap();
    fs::write(dir.join("a.RAF"), []).unwrap();
    fs::write(dir.join("ignored.fit"), []).unwrap();
    fs::create_dir(dir.join("nested.raf")).unwrap();

    let files = files_with_extensions(dir.path(), &["raf"]).unwrap();

    assert_eq!(names(&files), ["a.RAF", "z.raf"]);
    assert!(files.iter().all(|path| path.is_file()));
}

#[test]
fn readable_empty_directory_is_distinct_from_scan_failure() {
    let dir = TempDir::new("common-file-utils");
    assert_eq!(
        files_with_extensions(dir.path(), &["raf"]).unwrap(),
        Vec::<PathBuf>::new()
    );
}

#[test]
fn missing_directory_returns_contextual_error() {
    let dir = TempDir::new("common-file-utils");
    fs::remove_dir(dir.path()).unwrap();

    let error = files_with_extensions(dir.path(), &["raf"]).unwrap_err();

    assert_eq!(error.kind(), io::ErrorKind::NotFound);
    assert!(
        error
            .to_string()
            .contains(&dir.path().display().to_string())
    );
}

#[test]
fn file_instead_of_directory_returns_contextual_error() {
    let dir = TempDir::new("common-file-utils");
    let path = dir.join("frame.raf");
    fs::write(&path, []).unwrap();

    let error = files_with_extensions(&path, &["raf"]).unwrap_err();

    assert!(error.to_string().contains(&path.display().to_string()));
}

#[test]
fn publication_replaces_complete_files_and_cleans_up_failures() {
    let dir = TempDir::new("common-publication");
    let path = dir.join("state.bin");
    fs::write(&path, b"previous").unwrap();

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;

        fs::set_permissions(&path, fs::Permissions::from_mode(0o640)).unwrap();
    }

    publish_bytes(&path, b"durable", PublicationMode::Durable).unwrap();
    assert_eq!(fs::read(&path).unwrap(), b"durable");

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;

        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o640
        );
    }

    let error = publish(&path, PublicationMode::Cache, |file| {
        file.write_all(b"incomplete")?;
        Err(io::Error::other("injected write failure"))
    })
    .unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::Other);
    assert_eq!(
        fs::read(&path).unwrap(),
        b"durable",
        "a failed write preserves the prior complete file"
    );
    assert!(
        publication_temp_files(&path).is_empty(),
        "failed writes do not leave sibling temporary files"
    );

    let error = publish_with_replacement(
        &path,
        PublicationMode::Cache,
        |file| file.write_all(b"complete"),
        |_source, _destination, _mode| {
            Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "injected replacement failure",
            ))
        },
    )
    .unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
    assert_eq!(
        fs::read(&path).unwrap(),
        b"durable",
        "a failed replacement preserves the prior complete file"
    );
    assert!(
        publication_temp_files(&path).is_empty(),
        "failed replacement removes the completed temporary file"
    );

    let directory_target = dir.join("nonempty-directory");
    fs::create_dir_all(&directory_target).unwrap();
    fs::write(directory_target.join("keep"), b"old").unwrap();
    assert!(
        publish_bytes(&directory_target, b"new", PublicationMode::Cache).is_err(),
        "a file cannot replace a nonempty directory"
    );
    assert_eq!(fs::read(directory_target.join("keep")).unwrap(), b"old");
    assert!(
        publication_temp_files(&directory_target).is_empty(),
        "failed replacement does not leave a sibling temporary file"
    );

    let missing_parent = dir.join("missing-parent");
    let missing_target = missing_parent.join("state.bin");
    let error = publish_bytes(&missing_target, b"new", PublicationMode::Durable).unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::NotFound);
    assert!(
        !missing_parent.exists(),
        "publication does not silently create a missing destination directory"
    );

    let seek_target = dir.join("seek.bin");
    let mut file = SyncAtomicFile::new(&seek_target, PublicationMode::Cache).unwrap();
    file.write_all(b"header____body").unwrap();
    assert_eq!(file.seek(SeekFrom::Start(6)).unwrap(), 6);
    file.write_all(b"data").unwrap();
    file.commit_with_replacement(replace).unwrap();
    assert_eq!(fs::read(seek_target).unwrap(), b"headerdatabody");
}

#[tokio::test]
async fn two_phase_publication_commits_or_cleans_up() {
    let dir = TempDir::new("common-publication");
    let path = dir.join("two-phase.bin");
    fs::write(&path, b"previous").unwrap();

    let mut file = AtomicFile::new(&path, PublicationMode::Cache)
        .await
        .unwrap();
    file.write_all(b"complete").await.unwrap();
    file.commit().await.unwrap();
    assert_eq!(fs::read(&path).unwrap(), b"complete");
    assert!(publication_temp_files(&path).is_empty());

    let mut file = AtomicFile::new(&path, PublicationMode::Cache)
        .await
        .unwrap();
    file.write_all(b"incomplete").await.unwrap();
    drop(file);
    assert_eq!(fs::read(&path).unwrap(), b"complete");
    assert!(publication_temp_files(&path).is_empty());
}

#[test]
fn concurrent_publications_never_interleave() {
    let dir = TempDir::new("common-publication");
    let path = dir.join("concurrent.bin");
    let payloads = (0..8)
        .map(|value| vec![b'0' + value; 32 * 1024])
        .collect::<Vec<_>>();
    let barrier = Arc::new(Barrier::new(payloads.len()));
    let threads = payloads
        .iter()
        .cloned()
        .map(|payload| {
            let path = path.clone();
            let barrier = Arc::clone(&barrier);
            thread::spawn(move || {
                barrier.wait();
                publish_bytes(&path, &payload, PublicationMode::Cache).unwrap();
            })
        })
        .collect::<Vec<_>>();
    for thread in threads {
        thread.join().unwrap();
    }

    let published = fs::read(&path).unwrap();
    assert!(
        payloads.contains(&published),
        "the final file is exactly one writer's complete payload"
    );
    assert!(publication_temp_files(&path).is_empty());
}

#[cfg(unix)]
#[test]
fn unreadable_directory_returns_contextual_error() {
    use crate::Unreadable;

    let dir = TempDir::new("common-file-utils");
    let Some(unreadable) = Unreadable::new(dir.path()) else {
        return;
    };
    let result = files_with_extensions(dir.path(), &["raf"]);
    drop(unreadable);

    let error = result.unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
    assert!(
        error
            .to_string()
            .contains(&dir.path().display().to_string())
    );
}
