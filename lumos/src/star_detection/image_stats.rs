//! Per-pixel background and noise estimates.

use crate::common::Buffer2;

use super::buffer_pool::BufferPool;

/// Per-pixel background and noise estimates for an image.
///
/// Data-only struct containing the results of background estimation.
/// Used by subsequent pipeline stages for thresholding, centroid computation,
/// and SNR calculation.
#[derive(Debug)]
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

impl ImageStats {
    /// Release buffers back to the pool.
    pub fn release_to_pool(self, pool: &mut BufferPool) {
        pool.release_f32(self.background);
        pool.release_f32(self.noise);
        if let Some(adaptive) = self.adaptive_sigma {
            pool.release_f32(adaptive);
        }
    }
}
