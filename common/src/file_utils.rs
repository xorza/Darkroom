//! File utility functions for listing and filtering files.

use std::fs;
use std::path::{Path, PathBuf};

/// Supported RAW image file extensions.
pub const RAW_EXTENSIONS: &[&str] = &["raf", "cr2", "cr3", "nef", "arw", "dng"];

/// Supported FITS file extensions.
pub const FITS_EXTENSIONS: &[&str] = &["fit", "fits"];

/// Returns paths to all files in a directory matching the given extensions.
/// Extensions are matched case-insensitively. An unreadable directory (missing,
/// no permission, not a directory) yields an empty list rather than panicking.
pub fn files_with_extensions(dir: &Path, extensions: &[&str]) -> Vec<PathBuf> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };

    entries
        .filter_map(|e| e.ok())
        .filter(|e| {
            let path = e.path();
            if !path.is_file() {
                return false;
            }
            let ext = path.extension().and_then(|s| s.to_str()).unwrap_or("");
            extensions
                .iter()
                .any(|&expected| ext.eq_ignore_ascii_case(expected))
        })
        .map(|e| e.path())
        .collect()
}

/// Returns paths to all RAW and FITS image files in the given directory.
pub fn astro_image_files(dir: &Path) -> Vec<PathBuf> {
    let extensions: Vec<&str> = RAW_EXTENSIONS
        .iter()
        .chain(FITS_EXTENSIONS)
        .copied()
        .collect();
    files_with_extensions(dir, &extensions)
}

#[cfg(test)]
mod tests {
    use super::*;

    // The crate manifest dir is a stable, always-present fixture: it holds
    // `Cargo.toml` (a file) and `src/` (a subdir), exercised read-only below.
    fn crate_dir() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
    }

    fn names(paths: &[PathBuf]) -> Vec<String> {
        paths
            .iter()
            .map(|p| p.file_name().unwrap().to_str().unwrap().to_string())
            .collect()
    }

    #[test]
    fn missing_directory_returns_empty_instead_of_panicking() {
        let files = files_with_extensions(&crate_dir().join("no_such_dir_xyz"), &["rs"]);
        assert!(files.is_empty());
        assert!(astro_image_files(&crate_dir().join("no_such_dir_xyz")).is_empty());
    }

    #[test]
    fn passing_a_file_path_returns_empty() {
        // `read_dir` on a regular file errors; we must swallow it, not panic.
        let files = files_with_extensions(&crate_dir().join("Cargo.toml"), &["toml"]);
        assert!(files.is_empty());
    }

    #[test]
    fn matches_extension_and_skips_subdirectories() {
        let found = names(&files_with_extensions(&crate_dir(), &["toml"]));
        // `Cargo.toml` is the only top-level `.toml`; `src/` is a dir and excluded.
        assert_eq!(found, vec!["Cargo.toml"]);
    }

    #[test]
    fn extension_match_is_case_insensitive() {
        let upper = names(&files_with_extensions(&crate_dir(), &["TOML"]));
        assert!(upper.contains(&"Cargo.toml".to_string()));
    }

    #[test]
    fn unmatched_extension_returns_empty() {
        assert!(files_with_extensions(&crate_dir(), &["no_such_ext"]).is_empty());
    }

    #[test]
    fn only_regular_files_are_returned() {
        // Sweep every extension present at the crate root; `src` (a dir) and any
        // extensionless entry must never appear.
        let all = files_with_extensions(&crate_dir(), &["rs", "toml", "md", "lock"]);
        assert!(all.iter().all(|p| p.is_file()));
        assert!(!names(&all).contains(&"src".to_string()));
    }
}
