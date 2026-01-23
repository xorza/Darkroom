use std::env;
use std::path::PathBuf;

use crate::AstroImage;

/// Returns the calibration directory from LUMOS_CALIBRATION_DIR env var.
/// Returns None if not set. Prints a message to stderr if not set.
pub fn calibration_dir() -> Option<PathBuf> {
    match env::var("LUMOS_CALIBRATION_DIR") {
        Ok(dir) => Some(PathBuf::from(dir)),
        Err(_) => {
            eprintln!("LUMOS_CALIBRATION_DIR not set, skipping test");
            None
        }
    }
}

/// Returns the calibration_masters subdirectory within the calibration directory.
/// Returns None if LUMOS_CALIBRATION_DIR is not set or the subdirectory doesn't exist.
pub fn calibration_masters_dir() -> Option<PathBuf> {
    let cal_dir = calibration_dir()?;
    let masters_dir = cal_dir.join("calibration_masters");
    if masters_dir.exists() {
        Some(masters_dir)
    } else {
        eprintln!("calibration_masters subdirectory not found");
        None
    }
}

/// Loads all images from a subdirectory of the calibration directory.
/// Returns None if LUMOS_CALIBRATION_DIR is not set or the subdirectory doesn't exist.
pub fn load_calibration_images(subdir: &str) -> Option<Vec<AstroImage>> {
    let cal_dir = calibration_dir()?;
    let dir = cal_dir.join(subdir);

    if !dir.exists() {
        return None;
    }

    Some(AstroImage::load_from_directory(&dir))
}
