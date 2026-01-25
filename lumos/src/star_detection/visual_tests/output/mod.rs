//! Output utilities for visual tests.

#![allow(dead_code)]

pub mod comparison;
pub mod image_writer;
pub mod metrics;

// comparison module provides internal utilities used by image_writer
// save_comparison_png is re-exported from image_writer
pub use image_writer::*;
pub use metrics::*;
