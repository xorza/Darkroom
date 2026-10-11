//! The camera-RAW files of a directory.

use std::fs;
use std::path::{Path, PathBuf};

use crate::io::raw::RAW_EXTENSIONS;
use crate::io::raw::error::RawScanError;

/// The files in `dir` with a [`RAW_EXTENSIONS`] extension in any case, sorted by path.
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
        let is_raw = path
            .extension()
            .and_then(|value| value.to_str())
            .is_some_and(|extension| {
                RAW_EXTENSIONS
                    .iter()
                    .any(|raw| extension.eq_ignore_ascii_case(raw))
            });
        if !is_raw {
            continue;
        }
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
