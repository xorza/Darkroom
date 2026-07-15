//! Per-pixel background and noise estimates.

use imaginarium::Buffer2;

use crate::stacking::star_detection::buffer_pool::BufferPool;

/// Per-pixel background and noise estimates for an image.
///
/// Data-only struct containing the results of background estimation.
/// Used by subsequent pipeline stages for thresholding, centroid computation,
/// and SNR calculation.
#[derive(Debug)]
pub(crate) struct BackgroundEstimate {
    /// Per-pixel background values (sky level).
    pub background: Buffer2<f32>,
    /// Per-pixel noise (sigma) estimates.
    pub noise: Buffer2<f32>,
}

impl BackgroundEstimate {
    /// Release buffers back to the pool.
    pub(crate) fn release_to_pool(self, pool: &mut BufferPool) {
        pool.release_f32(self.background);
        pool.release_f32(self.noise);
    }
}
