//! Atomic same-directory publication, and the identity a cache keyed on a file compares.

pub(crate) mod file_identity;

use std::fs::{self, File, OpenOptions};
use std::io::{self, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::process;
use std::sync::atomic::{AtomicU64, Ordering};
use std::task::{Context, Poll};

use tokio::fs as tokio_fs;
use tokio::io::{AsyncSeek, AsyncWrite, AsyncWriteExt as _};
use tokio::task;

/// Whether publishing a file must survive an abrupt system shutdown.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PublicationMode {
    Durable,
    Cache,
}

#[derive(Debug)]
struct Publication {
    destination: PathBuf,
    temporary: PathBuf,
    mode: PublicationMode,
}

impl Publication {
    fn commit(mut self, file: File) -> io::Result<()> {
        // `seal` closes the handle on every path, before a failure drops `self` and removes the
        // temporary: Windows refuses to remove an open file.
        Self::seal(file, &self.destination, self.mode)?;
        replace(&self.temporary, &self.destination, self.mode)?;
        if self.mode == PublicationMode::Durable {
            let parent = self
                .destination
                .parent()
                .filter(|parent| !parent.as_os_str().is_empty())
                .unwrap_or_else(|| Path::new("."));
            sync_parent(parent)?;
        }
        self.temporary.clear();
        Ok(())
    }

    fn seal(mut file: File, destination: &Path, mode: PublicationMode) -> io::Result<()> {
        file.flush()?;
        prepare_destination(&file, destination)?;
        if mode == PublicationMode::Durable {
            file.sync_all()?;
        }
        Ok(())
    }
}

impl Drop for Publication {
    fn drop(&mut self) {
        if !self.temporary.as_os_str().is_empty() {
            let _ = fs::remove_file(&self.temporary);
        }
    }
}

#[derive(Debug)]
struct SyncAtomicFile {
    // The handle must close before `Publication` removes the path on Windows.
    file: File,
    publication: Publication,
}

impl SyncAtomicFile {
    /// Opens a new temporary beside `destination`; a name another writer holds is skipped.
    fn new(destination: &Path, mode: PublicationMode) -> io::Result<Self> {
        loop {
            let temporary = temporary_path(destination)?;
            match OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&temporary)
            {
                Ok(file) => {
                    return Ok(Self {
                        file,
                        publication: Publication {
                            destination: destination.to_path_buf(),
                            temporary,
                            mode,
                        },
                    });
                }
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
                Err(error) => return Err(error),
            }
        }
    }

    fn commit(self) -> io::Result<()> {
        let Self { file, publication } = self;
        publication.commit(file)
    }
}

impl Write for SyncAtomicFile {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.file.write(bytes)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.file.flush()
    }
}

impl Seek for SyncAtomicFile {
    fn seek(&mut self, position: SeekFrom) -> io::Result<u64> {
        self.file.seek(position)
    }
}

/// A Tokio file that atomically replaces its destination only when [`commit`](Self::commit)
/// succeeds. Dropping it removes the temporary file and preserves the existing destination.
#[derive(Debug)]
pub struct AtomicFile {
    // The handle must close before `Publication` removes the path on Windows.
    file: tokio_fs::File,
    publication: Publication,
}

impl AtomicFile {
    /// Create a writable same-directory temporary file for later atomic commit.
    pub async fn new(destination: &Path, mode: PublicationMode) -> io::Result<Self> {
        let destination = destination.to_path_buf();
        let SyncAtomicFile { file, publication } =
            task::spawn_blocking(move || SyncAtomicFile::new(&destination, mode))
                .await
                .expect("atomic-file create task panicked")?;
        Ok(Self {
            file: tokio_fs::File::from_std(file),
            publication,
        })
    }

    /// Atomically publish the completed file at its destination.
    pub async fn commit(mut self) -> io::Result<()> {
        self.file.flush().await?;
        let Self { file, publication } = self;
        let file = file.into_std().await;
        task::spawn_blocking(move || publication.commit(file))
            .await
            .expect("atomic-file commit task panicked")
    }
}

impl AsyncWrite for AtomicFile {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bytes: &[u8],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.file).poll_write(cx, bytes)
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.file).poll_flush(cx)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.file).poll_shutdown(cx)
    }
}

impl AsyncSeek for AtomicFile {
    fn start_seek(mut self: Pin<&mut Self>, position: SeekFrom) -> io::Result<()> {
        Pin::new(&mut self.file).start_seek(position)
    }

    fn poll_complete(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<u64>> {
        Pin::new(&mut self.file).poll_complete(cx)
    }
}

fn temporary_path(destination: &Path) -> io::Result<PathBuf> {
    static COUNTER: AtomicU64 = AtomicU64::new(0);

    let file_name = destination
        .file_name()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "path has no file name"))?;
    let sequence = COUNTER.fetch_add(1, Ordering::Relaxed);
    let mut temp_name = file_name.to_os_string();
    temp_name.push(format!(".{}.{sequence}.tmp", process::id()));
    Ok(destination.with_file_name(temp_name))
}

/// Publish bytes through a unique same-directory temporary file.
pub fn publish_bytes(path: &Path, bytes: &[u8], mode: PublicationMode) -> io::Result<()> {
    publish(path, mode, |file| file.write_all(bytes))
}

/// Publish a file through a unique same-directory temporary file.
///
/// Readers see either the previous complete file or the new complete file.
/// Durable publication synchronizes the file before replacement and the
/// directory entry after replacement. Cache publication skips those durability
/// barriers because a lost cache entry can be rebuilt.
pub fn publish(
    path: &Path,
    mode: PublicationMode,
    write: impl FnOnce(&mut File) -> io::Result<()>,
) -> io::Result<()> {
    let mut file = SyncAtomicFile::new(path, mode)?;
    write(&mut file.file)?;
    file.commit()
}

fn prepare_destination(file: &File, destination: &Path) -> io::Result<()> {
    match fs::metadata(destination) {
        Ok(metadata) if metadata.is_file() => {
            drop(OpenOptions::new().write(true).open(destination)?);
            file.set_permissions(metadata.permissions())
        }
        Ok(_) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

#[cfg(unix)]
fn replace(source: &Path, destination: &Path, _mode: PublicationMode) -> io::Result<()> {
    fs::rename(source, destination)
}

#[cfg(windows)]
#[expect(
    unsafe_code,
    reason = "`MoveFileExW` is the Win32 call that replaces a file in one step"
)]
fn replace(source: &Path, destination: &Path, mode: PublicationMode) -> io::Result<()> {
    use std::os::windows::ffi::OsStrExt as _;

    use windows_sys::Win32::Storage::FileSystem::{
        MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH, MoveFileExW,
    };

    let source = source
        .as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect::<Vec<_>>();
    let destination = destination
        .as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect::<Vec<_>>();
    let mut flags = MOVEFILE_REPLACE_EXISTING;
    if mode == PublicationMode::Durable {
        flags |= MOVEFILE_WRITE_THROUGH;
    }
    let succeeded = unsafe { MoveFileExW(source.as_ptr(), destination.as_ptr(), flags) };
    if succeeded == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

#[cfg(not(any(unix, windows)))]
fn replace(source: &Path, destination: &Path, _mode: PublicationMode) -> io::Result<()> {
    fs::rename(source, destination)
}

#[cfg(unix)]
fn sync_parent(parent: &Path) -> io::Result<()> {
    File::open(parent)?.sync_all()
}

#[cfg(not(unix))]
fn sync_parent(_parent: &Path) -> io::Result<()> {
    Ok(())
}

#[cfg(any(test, feature = "internals"))]
pub mod internals {
    use std::fs;
    use std::path::{Path, PathBuf};

    /// The publication temporaries beside `destination`: every file named
    /// like the ones a publication writes before it renames, which one that
    /// failed must not leave behind.
    pub fn publication_temp_files(destination: &Path) -> Vec<PathBuf> {
        let prefix = format!("{}.", destination.file_name().unwrap().to_string_lossy());
        fs::read_dir(destination.parent().unwrap())
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
}

#[cfg(test)]
mod tests;
