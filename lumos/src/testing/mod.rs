//! Testing utilities for lumos.

#![allow(dead_code)]

pub mod test_utils;

use std::path::PathBuf;

/// Returns the calibration directory from LUMOS_CALIBRATION_DIR env var.
/// Returns None if not set.
pub fn calibration_dir() -> Option<PathBuf> {
    std::env::var("LUMOS_CALIBRATION_DIR")
        .ok()
        .map(PathBuf::from)
}

/// Returns the calibration_masters subdirectory within the calibration directory.
/// Returns None if LUMOS_CALIBRATION_DIR is not set or the subdirectory doesn't exist.
pub fn calibration_masters_dir() -> Option<PathBuf> {
    let cal_dir = calibration_dir()?;
    let masters_dir = cal_dir.join("calibration_masters");
    if masters_dir.exists() {
        Some(masters_dir)
    } else {
        None
    }
}
