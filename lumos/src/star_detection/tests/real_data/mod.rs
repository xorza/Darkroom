//! Real-data tests for star detection.
//!
//! These tests require:
//! - `LUMOS_CALIBRATION_DIR` environment variable pointing to calibration data
//! - Network access (for survey benchmarks)
//! - External tools like `image2xy` from astrometry.net (for astrometry benchmarks)
//!
//! # Running real-data tests
//!
//! ```bash
//! cargo test -p lumos --features benchmark-tests real_data -- --ignored
//! ```
//!
//! # Environment variables
//!
//! - `LUMOS_CALIBRATION_DIR` - Path to calibration data directory containing:
//!   - `Lights/` - Light frames (RAW files)
//!   - `Darks/` - Dark frames
//!   - `Flats/` - Flat frames
//!   - `Bias/` - Bias frames
//!   - `calibrated_light.tiff` - A calibrated light frame
//!
//! - `LUMOS_TEST_CACHE_DIR` - Optional cache directory for test artifacts

pub mod astrometry_benchmark;
mod calibration_tests;
pub mod survey_benchmark;
