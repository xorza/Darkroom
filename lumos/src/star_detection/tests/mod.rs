//! Tests for star detection algorithms.
//!
//! This module is organized into:
//! - `unit_tests` - Unit tests for individual functions
//! - `synthetic/` - Tests using synthetic star fields (always run)
//! - `real_data/` - Tests requiring real calibration data (feature-gated)
//! - `common/` - Shared utilities for test output and visualization
//!
//! # Running tests
//!
//! ```bash
//! # Run all regular tests (unit + synthetic)
//! cargo test -p lumos star_detection::tests
//!
//! # Run real-data tests (requires LUMOS_CALIBRATION_DIR)
//! cargo test -p lumos --features real-data star_detection::tests::real_data -- --ignored
//! ```

pub mod common;
#[cfg(feature = "real-data")]
pub mod real_data;
pub mod synthetic;
mod unit_tests;
