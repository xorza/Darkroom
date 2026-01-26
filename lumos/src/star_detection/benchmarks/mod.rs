//! Benchmark and real-data tests for star detection.
//!
//! This module contains tests that require:
//! - Real calibration data (`LUMOS_CALIBRATION_DIR` environment variable)
//! - Network access (for survey benchmarks)
//! - External tools like `image2xy` from astrometry.net
//!
//! These tests are gated behind the `benchmark-tests` feature and are not
//! included in regular test runs.
//!
//! # Running benchmark tests
//!
//! ```bash
//! # Run all benchmark tests
//! cargo test -p lumos --features benchmark-tests -- --ignored
//!
//! # Run specific benchmark suite
//! cargo test -p lumos --features benchmark-tests real_data -- --ignored
//! cargo test -p lumos --features benchmark-tests astrometry -- --ignored
//! cargo test -p lumos --features benchmark-tests survey -- --ignored
//! ```

pub mod astrometry_benchmark;
mod real_data_tests;
pub mod survey_benchmark;
