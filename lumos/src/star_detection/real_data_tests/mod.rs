//! Real-data tests for star detection.
//!
//! This module contains tests that require:
//! - Real calibration data (`LUMOS_CALIBRATION_DIR` environment variable)
//! - Network access (for survey benchmarks)
//! - External tools like `image2xy` from astrometry.net
//!
//! These tests are gated behind the `benchmark-tests` feature and are not
//! included in regular test runs.
//!
//! # Running real-data tests
//!
//! ```bash
//! # Run all real-data tests (requires --ignored because tests are marked #[ignore])
//! cargo test -p lumos --features benchmark-tests real_data_tests -- --ignored
//!
//! # Run specific test suite
//! cargo test -p lumos --features benchmark-tests astrometry_benchmark -- --ignored
//! cargo test -p lumos --features benchmark-tests survey_benchmark -- --ignored
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
