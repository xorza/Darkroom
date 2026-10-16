//! [`ThresholdParams`]: the level a residual pixel must clear to be detected.

use imaginarium::Buffer2;
use rayon::prelude::*;

use crate::bit_buffer2::BitBuffer2;
use crate::simd::Kernel;
use crate::star_detection::threshold_mask::simd::ProcessWords;

/// What a threshold comparison needs beyond the buffers: how many σ above the noise a pixel must
/// sit, and the floor its σ is held to first.
///
/// Bundled rather than passed loose because they travel together through the entry points into the
/// kernel, the same way the background interpolator hands its kernel a `SplineSegment`. A residual
/// is detected where it exceeds `sigma · max(noise, min_noise)`.
#[derive(Debug, Clone, Copy)]
pub(crate) struct ThresholdParams {
    /// Detection threshold in units of the local noise σ.
    pub(crate) sigma: f32,
    /// Floor applied to each per-pixel σ, in the samples' own units — the frame's own floor,
    /// [`SkyNoise::floor`](crate::star_detection::background::sky_noise::SkyNoise::floor), which is
    /// what every caller in the pipeline passes.
    pub(crate) min_noise: f32,
}

impl ThresholdParams {
    /// The level a residual must exceed where the noise is `noise`: `sigma · max(noise,
    /// min_noise)`, the expression the kernel computes lane by lane.
    pub(crate) const fn level(self, noise: f32) -> f32 {
        self.sigma * noise.max(self.min_noise)
    }

    /// Fill `mask` from a residual — an image whose sky is already subtracted, matched-filtered or
    /// not: bit `i` set where `residual[i] > sigma · max(noise[i], min_noise)`.
    ///
    /// `residual` and `noise` have the mask's dimensions; the mask's row padding is left clear.
    pub(crate) fn fill_mask(
        self,
        residual: &Buffer2<f32>,
        noise: &Buffer2<f32>,
        mask: &mut BitBuffer2,
    ) {
        let width = mask.size.width;
        let height = mask.size.height;
        debug_assert_eq!(width, residual.width());
        debug_assert_eq!(height, residual.height());
        debug_assert_eq!(width, noise.width());
        debug_assert_eq!(height, noise.height());

        let residual = residual.pixels();
        let noise = noise.pixels();

        mask.par_rows_mut().enumerate().for_each(|(y, row_words)| {
            let row = y * width..(y + 1) * width;
            ProcessWords {
                pixels: &residual[row.clone()],
                noise: &noise[row],
                threshold: self,
                words: row_words,
            }
            .dispatch();
        });
    }
}

#[cfg(feature = "internals")]
pub(crate) mod internals {
    use crate::star_detection::threshold_mask::threshold_params::ThresholdParams;

    /// The σ floor this module's tests threshold with, shared by the kernel cross-checks in
    /// `threshold_mask::simd`. Frame-derived in production; fixed here so every case is graded on
    /// the noise it declares, and low enough never to bind on it.
    #[cfg(test)]
    const TEST_MIN_NOISE: f32 = 1e-6;

    impl ThresholdParams {
        /// The threshold at `sigma` with the shared test floor.
        #[cfg(test)]
        pub(crate) const fn test(sigma: f32) -> Self {
            Self {
                sigma,
                min_noise: TEST_MIN_NOISE,
            }
        }
    }
}
