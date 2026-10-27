use std::fs;
use std::fs::File;
use std::io;
use std::io::{Seek as _, SeekFrom, Write as _};
use std::sync::{Arc, Barrier};
use std::thread;

use tokio::io::AsyncWriteExt as _;

use crate::fs::{
    AtomicFile, PublicationMode, SyncAtomicFile, publish, publish_bytes, symlink, symlink_dir,
};
use crate::{FileSystem, TempDir};
use crate::{internals, platform};

#[test]
fn publication_replaces_complete_files_and_cleans_up_failures() {
    let dir = TempDir::new("common-publication");
    let path = dir.join("state.bin");
    fs::write(&path, b"previous").unwrap();

    let uncommon = platform::internals::set_uncommon_permissions(&path);

    // A handle open on the destination does not refuse the replace, and still reads the file it
    // opened. Windows' `MoveFileExW` refuses here, so this reaches the POSIX rename.
    let held = File::open(&path).unwrap();
    publish_bytes(&path, b"durable", PublicationMode::Durable).unwrap();
    assert_eq!(fs::read(&path).unwrap(), b"durable");
    assert_eq!(io::read_to_string(held).unwrap(), "previous");

    if let Some(uncommon) = uncommon {
        assert_eq!(fs::metadata(&path).unwrap().permissions(), uncommon);
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
        internals::publication_temp_files(&path).is_empty(),
        "failed writes do not leave sibling temporary files"
    );

    let dir_target = dir.join("nonempty-directory");
    fs::create_dir_all(&dir_target).unwrap();
    fs::write(dir_target.join("keep"), b"old").unwrap();
    assert!(
        publish_bytes(&dir_target, b"new", PublicationMode::Cache).is_err(),
        "a file cannot replace a nonempty directory"
    );
    assert_eq!(fs::read(dir_target.join("keep")).unwrap(), b"old");
    assert!(
        internals::publication_temp_files(&dir_target).is_empty(),
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
    file.commit().unwrap();
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
    assert!(internals::publication_temp_files(&path).is_empty());

    let mut file = AtomicFile::new(&path, PublicationMode::Cache)
        .await
        .unwrap();
    file.write_all(b"incomplete").await.unwrap();
    drop(file);
    assert_eq!(fs::read(&path).unwrap(), b"complete");
    assert!(internals::publication_temp_files(&path).is_empty());
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
    assert!(internals::publication_temp_files(&path).is_empty());
}

/// A folder link opens as the folder it names, and reads as a link itself; a file link opens as
/// its file. Both need the right to make links, which Windows grants only in Developer Mode or to
/// an elevated process.
#[test]
fn a_folder_link_opens_as_its_folder_and_a_file_link_as_its_file() {
    let dir = TempDir::new("common-links");
    if !FileSystem::symlinks_allowed(dir.path()) {
        return;
    }
    let folder = dir.join("folder");
    fs::create_dir(&folder).unwrap();
    fs::write(folder.join("inside.txt"), b"x").unwrap();
    symlink_dir(&folder, dir.join("folder-link")).unwrap();
    symlink(folder.join("inside.txt"), dir.join("file-link")).unwrap();

    let linked = dir.join("folder-link");
    assert!(fs::metadata(&linked).unwrap().is_dir());
    assert!(
        fs::symlink_metadata(&linked)
            .unwrap()
            .file_type()
            .is_symlink()
    );
    assert_eq!(fs::read(linked.join("inside.txt")).unwrap(), b"x");
    assert_eq!(fs::read(dir.join("file-link")).unwrap(), b"x");
}
