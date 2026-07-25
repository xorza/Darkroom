//! Output utilities for visual tests.

mod comparison;
pub(crate) mod image_writer;
pub(crate) mod metrics;

/// File extension for test output images.
/// Change this to switch the output format for all tests (e.g., "png", "tiff").
const TEST_OUTPUT_IMAGE_EXT: &str = "tiff";
