//! Output utilities for visual tests.

pub mod comparison;
pub mod image_writer;
pub mod metrics;

/// File extension for test output images.
/// Change this to switch the output format for all tests (e.g., "png", "tiff").
pub const TEST_OUTPUT_IMAGE_EXT: &str = "tiff";
