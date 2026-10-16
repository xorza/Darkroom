//! Per-pixel background and noise estimates.

use imaginarium::Buffer2;
use rayon::prelude::*;

use crate::background_mesh::tile_stats::TileComponent;
use crate::concurrency::SAMPLES_PER_BLOCK;
use crate::image::masked_plane::MaskedPlane;
use crate::scratch_pool::{Pooled, ScratchPool};
use crate::star_detection::background::sky_noise::{MeshReaders, SkyNoise};
use crate::star_detection::background::workspace::BackgroundWorkspace;
use crate::star_detection::star_detection_config::background_config::BackgroundConfig;

/// Per-pixel background and noise estimates for an image: the sky, and its noise as the stages
/// after its removal keep it.
#[derive(Debug)]
pub(crate) struct BackgroundEstimate {
    /// Per-pixel background values (sky level).
    pub(crate) background: Pooled<Buffer2<f32>>,
    pub(crate) sky: SkyNoise,
}

impl BackgroundEstimate {
    /// Estimate background and noise for the plane, leaving out the pixels its mask sets: the
    /// tiles' sigma-clipped statistics, then the mesh's spline through them, per component.
    pub(crate) fn estimate(
        plane: MaskedPlane<'_>,
        config: &BackgroundConfig,
        planes: &ScratchPool<Buffer2<f32>>,
        workspace: &mut BackgroundWorkspace,
    ) -> Self {
        let MaskedPlane { pixels, mask } = plane;
        let mesh = workspace
            .mesh
            .measure(pixels, mask, None, config.mesh_params());
        let mut background = planes.acquire();
        workspace
            .spline
            .interpolate(mesh, TileComponent::Sky, &mut background);
        let sky = SkyNoise::of_mesh(
            mesh,
            planes,
            MeshReaders {
                spline: &mut workspace.spline,
                values: &mut workspace.values,
            },
        );
        Self { background, sky }
    }

    /// `pixels` less this background, into `residual`.
    pub(crate) fn residual_into(&self, pixels: &Buffer2<f32>, residual: &mut Buffer2<f32>) {
        residual
            .pixels_mut()
            .par_chunks_mut(SAMPLES_PER_BLOCK)
            .zip(pixels.pixels().par_chunks(SAMPLES_PER_BLOCK))
            .zip(self.background.pixels().par_chunks(SAMPLES_PER_BLOCK))
            .for_each(|((out, values), sky)| {
                for ((out, &value), &sky) in out.iter_mut().zip(values).zip(sky) {
                    *out = value - sky;
                }
            });
    }

    /// Subtract the sky from `pixels` in place, leaving the residual every later stage reads, and
    /// keep only its noise: the background plane goes back to the pool.
    pub(crate) fn subtract_from(self, pixels: &mut Buffer2<f32>) -> SkyNoise {
        pixels
            .pixels_mut()
            .par_chunks_mut(SAMPLES_PER_BLOCK)
            .zip(self.background.pixels().par_chunks(SAMPLES_PER_BLOCK))
            .for_each(|(values, sky)| {
                for (value, &sky) in values.iter_mut().zip(sky) {
                    *value -= sky;
                }
            });
        self.sky
    }
}

#[cfg(test)]
pub(crate) mod internals {
    use imaginarium::Buffer2;

    use crate::math::size2us::Size2us;
    use crate::scratch_pool::Pooled;
    use crate::star_detection::background::background_estimate::BackgroundEstimate;
    use crate::star_detection::background::sky_noise::SkyNoise;

    impl BackgroundEstimate {
        /// `pixels` less this background: the residual the stages after the subtraction read.
        pub(crate) fn residual_of(&self, pixels: &Buffer2<f32>) -> Buffer2<f32> {
            let mut residual = pixels.clone();
            for (value, &sky) in residual
                .pixels_mut()
                .iter_mut()
                .zip(self.background.pixels())
            {
                *value -= sky;
            }
            residual
        }

        /// This estimate's noise, as [`BackgroundEstimate::subtract_from`] hands it on.
        pub(crate) fn sky_noise(&self) -> SkyNoise {
            let noise = &self.sky.noise;
            let size = Size2us::new(noise.width(), noise.height());
            SkyNoise {
                noise: Pooled::loose(Buffer2::clone(noise), size),
                floor: self.sky.floor,
            }
        }
    }
}
