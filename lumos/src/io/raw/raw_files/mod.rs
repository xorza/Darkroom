//! The camera-RAW files of a directory.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use crate::io::raw::RAW_EXTENSIONS;

/// The files in `dir` with a [`RAW_EXTENSIONS`] extension in any case, sorted by path.
/// Subdirectories are not entered. A directory, entry or metadata error names the path it is
/// about.
pub fn raw_files(dir: &Path) -> io::Result<Vec<PathBuf>> {
    let entries = fs::read_dir(dir).map_err(|error| {
        path_error(
            &format!("failed to read directory '{}'", dir.display()),
            &error,
        )
    })?;
    let mut files = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|error| {
            path_error(
                &format!("failed to read entry in directory '{}'", dir.display()),
                &error,
            )
        })?;
        let path = entry.path();
        let metadata = fs::metadata(&path).map_err(|error| {
            path_error(
                &format!("failed to read metadata for '{}'", path.display()),
                &error,
            )
        })?;
        let is_raw = path
            .extension()
            .and_then(|value| value.to_str())
            .is_some_and(|extension| {
                RAW_EXTENSIONS
                    .iter()
                    .any(|raw| extension.eq_ignore_ascii_case(raw))
            });
        if metadata.is_file() && is_raw {
            files.push(path);
        }
    }
    files.sort();
    Ok(files)
}

fn path_error(message: &str, source: &io::Error) -> io::Error {
    io::Error::new(source.kind(), format!("{message}: {source}"))
}

#[cfg(test)]
mod tests;
