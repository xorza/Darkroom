//! Output utilities for visual tests.

#![allow(dead_code)]

pub mod comparison;
pub mod image_writer;
pub mod metrics;

pub use image_writer::*;
pub use metrics::*;

/// File extension for test output images.
/// Change this to switch the output format for all tests (e.g., "png", "tiff").
pub const TEST_OUTPUT_IMAGE_EXT: &str = "tiff";
