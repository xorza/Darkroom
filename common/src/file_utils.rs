//! File utility functions for listing and filtering files.

use std::fs;
use std::path::{Path, PathBuf};

/// Supported RAW image file extensions.
pub const RAW_EXTENSIONS: &[&str] = &["raf", "cr2", "cr3", "nef", "arw", "dng"];

/// Supported FITS file extensions.
pub const FITS_EXTENSIONS: &[&str] = &["fit", "fits"];

/// Returns paths to all files in a directory matching the given extensions.
/// Extensions are matched case-insensitively.
pub fn files_with_extensions(dir: &Path, extensions: &[&str]) -> Vec<PathBuf> {
    if !dir.exists() {
        return Vec::new();
    }

    fs::read_dir(dir)
        .expect("Failed to read directory")
        .filter_map(|e| e.ok())
        .filter(|e| {
            let path = e.path();
            if !path.is_file() {
                return false;
            }
            let ext = path.extension().and_then(|s| s.to_str()).unwrap_or("");
            extensions.contains(&ext.to_lowercase().as_str())
        })
        .map(|e| e.path())
        .collect()
}

/// Returns paths to all RAW and FITS image files in the given directory.
pub fn astro_image_files(dir: &Path) -> Vec<PathBuf> {
    let extensions: Vec<&str> = RAW_EXTENSIONS
        .iter()
        .chain(FITS_EXTENSIONS.iter())
        .copied()
        .collect();
    files_with_extensions(dir, &extensions)
}
