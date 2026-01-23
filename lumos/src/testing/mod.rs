//! Testing utilities for lumos.

#![allow(dead_code)]

use std::path::PathBuf;

use crate::AstroImage;

/// Returns the calibration directory from LUMOS_CALIBRATION_DIR env var.
/// Prints a message if not set.
pub fn calibration_dir() -> Option<PathBuf> {
    match std::env::var("LUMOS_CALIBRATION_DIR")
        .ok()
        .map(PathBuf::from)
    {
        Some(dir) => Some(dir),
        None => {
            eprintln!("LUMOS_CALIBRATION_DIR not set, skipping test");
            None
        }
    }
}

/// Returns the calibration_masters subdirectory within the calibration directory.
/// Prints a message if not found.
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

/// Initialize tracing subscriber for tests.
/// Safe to call multiple times - will only initialize once.
/// Respects RUST_LOG env var, defaults to "info".
pub fn init_tracing() {
    use tracing_subscriber::EnvFilter;
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_test_writer()
        .try_init();
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
