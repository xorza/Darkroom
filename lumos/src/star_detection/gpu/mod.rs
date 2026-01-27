//! GPU-accelerated star detection operations.
//!
//! This module provides GPU acceleration for the threshold detection phase
//! of star detection. Uses a hybrid approach:
//! - GPU: Threshold mask creation and dilation
//! - CPU: Connected component labeling (optimal for sparse astronomical images)
//!
//! # Usage
//!
//! For best performance when processing multiple images, reuse the detector:
//!
//! ```ignore
//! let mut detector = GpuThresholdDetector::new();
//! if detector.gpu_available() {
//!     for image in images {
//!         let candidates = detect_stars_gpu_with_detector(
//!             &mut detector, &pixels, width, height, &background, &config
//!         );
//!     }
//! }
//! ```
//!
//! Note: This module provides an optional GPU-accelerated path for star detection.
//! The main `find_stars()` function uses CPU-based detection which is sufficient
//! for most use cases. GPU detection should be used when processing many images
//! in batch or when GPU resources are available and CPU is a bottleneck.

// These items are public API for optional GPU-accelerated star detection.
// The main CPU-based detection is still used by default in find_stars().
#[allow(unused_imports)]
mod pipeline;
mod threshold;

#[allow(unused_imports)]
pub use threshold::{
    GpuThresholdConfig, GpuThresholdDetector, MAX_DILATION_RADIUS, detect_stars_gpu,
    detect_stars_gpu_with_detector,
};
