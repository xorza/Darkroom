//! Test utilities for lumos.

use std::path::PathBuf;

use crate::AstroImage;

use crate::testing::{calibration_dir, calibration_masters_dir};

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

/// Returns the calibration directory, printing a message if not set.
/// This is a wrapper around common::calibration_dir that adds user feedback.
pub fn calibration_dir_with_message() -> Option<PathBuf> {
    match calibration_dir() {
        Some(dir) => Some(dir),
        None => {
            eprintln!("LUMOS_CALIBRATION_DIR not set, skipping test");
            None
        }
    }
}

/// Returns the calibration_masters subdirectory, printing a message if not found.
pub fn calibration_masters_dir_with_message() -> Option<PathBuf> {
    match calibration_masters_dir() {
        Some(dir) => Some(dir),
        None => {
            eprintln!("calibration_masters subdirectory not found");
            None
        }
    }
}

/// Loads all images from a subdirectory of the calibration directory.
/// Returns None if LUMOS_CALIBRATION_DIR is not set or the subdirectory doesn't exist.
pub fn load_calibration_images(subdir: &str) -> Option<Vec<AstroImage>> {
    let cal_dir = calibration_dir_with_message()?;
    let dir = cal_dir.join(subdir);

    if !dir.exists() {
        return None;
    }

    Some(AstroImage::load_from_directory(&dir))
}
