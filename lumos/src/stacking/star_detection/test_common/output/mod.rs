//! Output utilities for visual tests.

mod comparison;
pub(in crate::stacking::star_detection) mod image_writer;
pub(in crate::stacking::star_detection) mod metrics;

/// File extension for test output images.
/// Change this to switch the output format for all tests (e.g., "png", "tiff").
const TEST_OUTPUT_IMAGE_EXT: &str = "tiff";
