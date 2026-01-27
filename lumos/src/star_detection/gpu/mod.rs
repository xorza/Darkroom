//! GPU-accelerated star detection operations.
//!
//! This module provides GPU acceleration for the threshold detection phase
//! of star detection. Uses a hybrid approach:
//! - GPU: Threshold mask creation and dilation
//! - CPU: Connected component labeling (optimal for sparse astronomical images)

// Allow dead_code for now - this module is implemented but not yet integrated
// into the main star detection pipeline. Integration will come in subsequent tasks.
#[allow(dead_code)]
mod pipeline;
#[allow(dead_code)]
mod threshold;

#[allow(unused_imports)]
pub use threshold::{GpuThresholdConfig, GpuThresholdDetector, MAX_DILATION_RADIUS};
