//! File discovery and atomic same-directory publication.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Write as _};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

/// Whether publishing a file must survive an abrupt system shutdown.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PublicationMode {
    Durable,
    Cache,
}

#[derive(Debug)]
struct PendingPublication {
    path: PathBuf,
    file: Option<File>,
}

impl PendingPublication {
    fn close(&mut self) {
        drop(self.file.take());
    }
}

impl Drop for PendingPublication {
    fn drop(&mut self) {
        self.close();
        let _ = fs::remove_file(&self.path);
    }
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
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));

    let mut pending = create_pending(path)?;
    write(pending.file.as_mut().expect("pending file is open"))?;
    prepare_destination(pending.file.as_ref().expect("pending file is open"), path)?;
    if mode == PublicationMode::Durable {
        pending
            .file
            .as_ref()
            .expect("pending file is open")
            .sync_all()?;
    }
    pending.close();

    replacement(&pending.path, path, mode)?;
    if mode == PublicationMode::Durable {
        sync_parent(parent)?;
    }
    Ok(())
}

fn create_pending(path: &Path) -> io::Result<PendingPublication> {
    static COUNTER: AtomicU64 = AtomicU64::new(0);

    let file_name = path
        .file_name()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "path has no file name"))?;
    loop {
        let sequence = COUNTER.fetch_add(1, Ordering::Relaxed);
        let mut temp_name = file_name.to_os_string();
        temp_name.push(format!(".{}.{sequence}.tmp", std::process::id()));
        let temp_path = path.with_file_name(temp_name);
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp_path)
        {
            Ok(file) => {
                return Ok(PendingPublication {
                    path: temp_path,
                    file: Some(file),
                });
            }
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error),
        }
    }
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
