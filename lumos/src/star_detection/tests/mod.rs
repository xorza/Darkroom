//! Tests for star detection algorithms.
//!
//! This module is organized into:
//! - `unit_tests` - Unit tests for individual functions
//! - `synthetic/` - Tests using synthetic star fields (always run)
//! - `common/` - Shared utilities for test output and visualization
//!
//! Real data tests are located in `crate::testing::real_data`.
//!
//! # Running tests
//!
//! ```bash
//! # Run all regular tests (unit + synthetic)
//! cargo test -p lumos star_detection::tests
//!
//! # Run real-data tests (requires LUMOS_CALIBRATION_DIR)
//! cargo test -p lumos --features real-data testing::real_data -- --ignored
//! ```

pub mod common;
pub mod synthetic;
