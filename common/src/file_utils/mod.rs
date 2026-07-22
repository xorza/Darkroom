//! File discovery and atomic same-directory publication.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::task::{Context, Poll};

use tokio::io::{AsyncSeek, AsyncWrite, AsyncWriteExt as _};

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
    fn commit_with_replacement(
        mut self,
        mut file: File,
        replacement: impl FnOnce(&Path, &Path, PublicationMode) -> io::Result<()>,
    ) -> io::Result<()> {
        let parent = self
            .destination
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."));

        if let Err(error) = file.flush() {
            drop(file);
            return Err(error);
        }
        if let Err(error) = prepare_destination(&file, &self.destination) {
            drop(file);
            return Err(error);
        }
        if self.mode == PublicationMode::Durable
            && let Err(error) = file.sync_all()
        {
            drop(file);
            return Err(error);
        }
        drop(file);

        replacement(&self.temporary, &self.destination, self.mode)?;
        if self.mode == PublicationMode::Durable {
            sync_parent(parent)?;
        }
        self.temporary.clear();
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

    fn commit_with_replacement(
        self,
        replacement: impl FnOnce(&Path, &Path, PublicationMode) -> io::Result<()>,
    ) -> io::Result<()> {
        let Self { file, publication } = self;
        publication.commit_with_replacement(file, replacement)
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
    file: tokio::fs::File,
    publication: Publication,
}

impl AtomicFile {
    /// Create a writable same-directory temporary file for later atomic commit.
    pub async fn new(destination: &Path, mode: PublicationMode) -> io::Result<Self> {
        loop {
            let temporary = temporary_path(destination)?;
            match tokio::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&temporary)
                .await
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

    /// Atomically publish the completed file at its destination.
    pub async fn commit(mut self) -> io::Result<()> {
        self.file.flush().await?;
        let Self { file, publication } = self;
        let file = file.into_std().await;
        tokio::task::spawn_blocking(move || publication.commit_with_replacement(file, replace))
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
    temp_name.push(format!(".{}.{sequence}.tmp", std::process::id()));
    Ok(destination.with_file_name(temp_name))
}

/// Returns sorted paths to all files in a directory matching the given extensions.
///
/// Extensions are matched case-insensitively. Directory, entry, and metadata
/// errors are returned with the affected path.
pub fn files_with_extensions(dir: &Path, extensions: &[&str]) -> io::Result<Vec<PathBuf>> {
    let entries = fs::read_dir(dir).map_err(|error| {
        path_error(
            format!("failed to read directory '{}'", dir.display()),
            error,
        )
    })?;
    let mut files = Vec::new();

    for entry in entries {
        let entry = entry.map_err(|error| {
            path_error(
                format!("failed to read entry in directory '{}'", dir.display()),
                error,
            )
        })?;
        let path = entry.path();
        let metadata = fs::metadata(&path).map_err(|error| {
            path_error(
                format!("failed to read metadata for '{}'", path.display()),
                error,
            )
        })?;
        if !metadata.is_file() {
            continue;
        }
        let extension = path
            .extension()
            .and_then(|value| value.to_str())
            .unwrap_or("");
        if extensions
            .iter()
            .any(|expected| extension.eq_ignore_ascii_case(expected))
        {
            files.push(path);
        }
    }

    files.sort();
    Ok(files)
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
    publish_with_replacement(path, mode, write, replace)
}

fn publish_with_replacement(
    path: &Path,
    mode: PublicationMode,
    write: impl FnOnce(&mut File) -> io::Result<()>,
    replacement: impl FnOnce(&Path, &Path, PublicationMode) -> io::Result<()>,
) -> io::Result<()> {
    let mut file = SyncAtomicFile::new(path, mode)?;
    write(&mut file.file)?;
    file.commit_with_replacement(replacement)
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

fn path_error(message: String, source: io::Error) -> io::Error {
    io::Error::new(source.kind(), format!("{message}: {source}"))
}

#[cfg(test)]
mod tests;
