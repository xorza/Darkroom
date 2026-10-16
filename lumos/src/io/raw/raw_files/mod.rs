//! The camera-RAW files of a directory.

use std::fs;
use std::path::{Path, PathBuf};

use crate::io::input_format::InputFormat;
use crate::io::raw::error::RawScanError;

/// The files in `dir` with a [`RAW_EXTENSIONS`](crate::RAW_EXTENSIONS) extension in any case, sorted by path.
/// Subdirectories are not entered. An entry is examined only when its name is a RAW file's, so
/// nothing else in the directory, a dangling link included, can fail the scan; a link so named is
/// followed, and kept when it leads to a file.
///
/// # Errors
///
/// [`RawScanError`], naming the directory or the entry it is about.
pub fn raw_files(dir: &Path) -> Result<Vec<PathBuf>, RawScanError> {
    let entries = fs::read_dir(dir).map_err(|source| RawScanError::Directory {
        path: dir.to_path_buf(),
        source,
    })?;
    let mut files = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|source| RawScanError::Entry {
            path: dir.to_path_buf(),
            source,
        })?;
        let path = entry.path();
        if !matches!(InputFormat::of(&path), Ok(InputFormat::CameraRaw)) {
            continue;
        }
        #[expect(
            clippy::filetype_is_file,
            reason = "a raw file is a regular file, and a fifo or a device is not one"
        )]
        let examined = entry.file_type().and_then(|file_type| {
            if file_type.is_symlink() {
                fs::metadata(&path).map(|metadata| metadata.is_file())
            } else {
                Ok(file_type.is_file())
            }
        });
        match examined {
            Ok(true) => files.push(path),
            Ok(false) => {}
            Err(source) => return Err(RawScanError::File { path, source }),
        }
    }
    files.sort();
    Ok(files)
}

#[cfg(test)]
mod tests;
