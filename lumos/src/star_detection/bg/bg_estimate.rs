//! Per-pixel background and noise estimates.

use common::par;
use imaginarium::Buffer2;
use rayon::prelude::*;

use crate::bg_mesh::mesh_surface::MeshSurface;
use crate::bg_mesh::tile_stats::TileComponent;
use crate::image::masked_plane::MaskedPlane;
use crate::scratch_pool::{Pooled, ScratchPool};
use crate::star_detection::bg::sky_noise::{MeshReaders, SkyNoise};
use crate::star_detection::bg::workspace::BgWorkspace;
use crate::star_detection::star_detection_config::bg_config::BgConfig;

/// Per-pixel background and noise estimates for an image: the sky, and its noise as the stages
/// after its removal keep it, each read a row at a time.
#[derive(Debug)]
pub(crate) struct BgEstimate {
    /// Per-pixel background values (sky level).
    pub(crate) bg: Pooled<MeshSurface>,
    pub(crate) sky: SkyNoise,
}

impl BgEstimate {
    /// Estimate background and noise for the plane, leaving out the pixels its mask sets: the
    /// tiles' sigma-clipped statistics, then the mesh's spline through them, per component.
    pub(crate) fn estimate(
        plane: MaskedPlane<'_>,
        config: &BgConfig,
        surfaces: &ScratchPool<MeshSurface>,
        workspace: &mut BgWorkspace,
    ) -> Self {
        let MaskedPlane { pixels, mask } = plane;
        let mesh = workspace
            .mesh
            .measure(pixels, mask, None, config.mesh_params());
        let mut bg = surfaces.acquire();
        workspace.spline.solve(mesh, TileComponent::Sky, &mut bg);
        let sky = SkyNoise::of_mesh(
            mesh,
            surfaces,
            MeshReaders {
                spline: &mut workspace.spline,
                values: &mut workspace.values,
            },
        );
        Self { bg, sky }
    }

    /// Subtract the sky from `pixels` in place, leaving the residual every later stage reads, and
    /// keep only its noise: the background surface goes back to the pool.
    pub(crate) fn subtract_from(self, pixels: &mut Buffer2<f32>) -> SkyNoise {
        let width = pixels.width();
        par::chunks_mut(pixels.pixels_mut(), width)
            .enumerate()
            .for_each_init(
                || self.bg.reader(),
                |bg, (y, values)| {
                    for (value, &sky) in values.iter_mut().zip(bg.row(y)) {
                        *value -= sky;
                    }
                },
            );
        self.sky
    }
}

#[cfg(test)]
pub(crate) mod internals {
    use imaginarium::Buffer2;

    use crate::scratch_pool::Pooled;
    use crate::star_detection::bg::bg_estimate::BgEstimate;
    use crate::star_detection::bg::sky_noise::SkyNoise;

    impl BgEstimate {
        /// This estimate's noise, as [`BgEstimate::subtract_from`] hands it on.
        pub(crate) fn sky_noise(&self) -> SkyNoise {
            SkyNoise {
                noise: Pooled::loose(self.sky.noise.duplicate(), ()),
                floor: self.sky.floor,
            }
        }

        /// `pixels` less this background: the residual the stages after the subtraction read.
        pub(crate) fn residual_of(&self, pixels: &Buffer2<f32>) -> Buffer2<f32> {
            let mut residual = pixels.clone();
            let mut sky = self.bg.reader();
            for (y, row) in residual.pixels_mut().chunks_mut(pixels.width()).enumerate() {
                for (value, &level) in row.iter_mut().zip(sky.row(y)) {
                    *value -= level;
                }
            }
            residual
        }
    }
}
