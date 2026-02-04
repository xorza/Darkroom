//! Per-pixel background and noise estimates.

use crate::common::Buffer2;

/// Per-pixel background and noise estimates for an image.
///
/// Data-only struct containing the results of background estimation.
/// Used by subsequent pipeline stages for thresholding, centroid computation,
/// and SNR calculation.
#[derive(Debug)]
#[allow(dead_code)] // Will be used in stages/background.rs (Step 4)
pub struct ImageStats {
    /// Per-pixel background values (sky level).
    pub background: Buffer2<f32>,
    /// Per-pixel noise (sigma) estimates.
    pub noise: Buffer2<f32>,
    /// Per-pixel adaptive detection threshold in sigma units.
    /// Higher in nebulous/high-contrast regions, lower in uniform sky.
    /// Only populated when adaptive thresholding is enabled.
    pub adaptive_sigma: Option<Buffer2<f32>>,
}
