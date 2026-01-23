use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use crate::AstroImage;

/// Returns the calibration directory from LUMOS_CALIBRATION_DIR env var.
/// Returns None if not set.
fn calibration_dir() -> Option<PathBuf> {
    env::var("LUMOS_CALIBRATION_DIR").ok().map(PathBuf::from)
}

/// Returns paths to all RAW image files in the given directory.
/// Supports: RAF, CR2, CR3, NEF, ARW, DNG
fn raw_files_in_dir(dir: &Path) -> Vec<PathBuf> {
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
            matches!(
                ext.to_lowercase().as_str(),
                "raf" | "cr2" | "cr3" | "nef" | "arw" | "dng"
            )
        })
        .map(|e| e.path())
        .collect()
}

/// Loads all RAW images from the given directory.
fn load_raw_images(dir: &Path) -> Vec<AstroImage> {
    raw_files_in_dir(dir)
        .iter()
        .map(|path| AstroImage::from_file(path).expect("Failed to load image"))
        .collect()
}

/// Loads all images from a subdirectory of the calibration directory.
/// Returns None if LUMOS_CALIBRATION_DIR is not set or the subdirectory doesn't exist.
pub fn load_calibration_images(subdir: &str) -> Option<Vec<AstroImage>> {
    let cal_dir = calibration_dir()?;
    let dir = cal_dir.join(subdir);

    if !dir.exists() {
        return None;
    }

    Some(load_raw_images(&dir))
}
