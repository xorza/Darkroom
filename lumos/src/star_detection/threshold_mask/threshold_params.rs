//! [`ThresholdParams`]: the level a residual pixel must clear to be detected.

use imaginarium::Buffer2;
use rayon::prelude::*;

use crate::bg_mesh::mesh_surface::{MeshSurface, SurfaceReader};
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
    /// [`SkyNoise::floor`](crate::star_detection::bg::sky_noise::SkyNoise::floor), which is
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
    /// not: bit `i` set where `residual[i] > sigma · max(noise[i], min_noise)`, the noise read a
    /// row at a time off its surface.
    ///
    /// `residual` and `noise` have the mask's dimensions; the mask's row padding is left clear.
    pub(crate) fn fill_mask(
        self,
        residual: &Buffer2<f32>,
        noise: &MeshSurface,
        mask: &mut BitBuffer2,
    ) {
        self.fill_mask_from(residual, mask, || noise.reader());
    }

    /// [`Self::fill_mask`] with the noise of each row from a reader `reader` makes for it.
    fn fill_mask_from<R: NoiseRows>(
        self,
        residual: &Buffer2<f32>,
        mask: &mut BitBuffer2,
        reader: impl Fn() -> R + Sync + Send,
    ) {
        let width = mask.size.width;
        debug_assert_eq!(width, residual.width());
        debug_assert_eq!(mask.size.height, residual.height());
        mask.par_rows_mut().enumerate().for_each(|(y, row_words)| {
            let mut noise = reader();
            ProcessWords {
                pixels: residual.row(y),
                noise: noise.row(y),
                threshold: self,
                words: row_words,
            }
            .dispatch();
        });
    }
}

/// What a threshold reads each row's noise from.
trait NoiseRows {
    fn row(&mut self, y: usize) -> &[f32];
}

impl NoiseRows for SurfaceReader<'_> {
    fn row(&mut self, y: usize) -> &[f32] {
        Self::row(self, y)
    }
}

#[cfg(feature = "internals")]
pub(crate) mod internals {
    #[cfg(test)]
    use imaginarium::Buffer2;

    #[cfg(test)]
    use crate::bit_buffer2::BitBuffer2;
    #[cfg(test)]
    use crate::star_detection::threshold_mask::threshold_params::NoiseRows;
    use crate::star_detection::threshold_mask::threshold_params::ThresholdParams;

    /// A noise plane read a row at a time.
    #[cfg(test)]
    struct PlaneRows<'a>(&'a Buffer2<f32>);

    #[cfg(test)]
    impl NoiseRows for PlaneRows<'_> {
        fn row(&mut self, y: usize) -> &[f32] {
            self.0.row(y)
        }
    }

    /// The σ floor this module's tests threshold with, shared by the kernel cross-checks in
    /// `threshold_mask::simd`. Frame-derived in production; fixed here so every case is graded on
    /// the noise it declares, and low enough never to bind on it.
    #[cfg(test)]
    const TEST_MIN_NOISE: f32 = 1e-6;

    impl ThresholdParams {
        /// [`ThresholdParams::fill_mask`] over a noise plane of any values, which no surface
        /// holds.
        #[cfg(test)]
        pub(crate) fn fill_mask_plane(
            self,
            residual: &Buffer2<f32>,
            noise: &Buffer2<f32>,
            mask: &mut BitBuffer2,
        ) {
            self.fill_mask_from(residual, mask, || PlaneRows(noise));
        }

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
