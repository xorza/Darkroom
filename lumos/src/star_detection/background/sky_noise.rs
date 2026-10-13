//! [`SkyNoise`]: what detection keeps of the sky once it is subtracted.

use imaginarium::Buffer2;

use crate::image::masked_plane::MaskedPlane;
use crate::math::vec2us::Vec2us;
use crate::scratch_pool::Pooled;
use crate::star_detection::background::background_estimate::{self, Planes};
use crate::star_detection::resources::DetectionResources;
use crate::star_detection::star_detection_config::background_config::BackgroundConfig;

/// The sky's noise per pixel and its floor: all the stages after the sky's removal need of it.
///
/// The sky level itself went into the residual plane `pixels − background` that those stages read,
/// so none of them can see sky-included values — the threshold, the labeling, both deblenders and
/// the measurement all work on the source alone.
#[derive(Debug)]
pub(crate) struct SkyNoise {
    /// Per-pixel noise (σ).
    pub(crate) noise: Pooled<Buffer2<f32>>,
    /// The floor every threshold built from [`Self::noise`] applies to it — see
    /// `background_estimate::noise_floor_from`.
    pub(crate) floor: f32,
}

impl SkyNoise {
    /// The noise of `plane` alone, by the background estimate's mesh, leaving out the pixels its
    /// mask sets: for a plane whose sky is already out, whose σ is all a threshold needs.
    pub(crate) fn measure(
        plane: MaskedPlane<'_>,
        config: &BackgroundConfig,
        resources: &mut DetectionResources,
    ) -> Self {
        let MaskedPlane { pixels, mask } = plane;
        let mut noise = resources.acquire_f32();
        let workspace = &mut resources.background;
        let tile_grid = workspace.mesh.compute(pixels, mask, config.mesh_params());
        let floor = background_estimate::noise_floor_from(tile_grid, &mut resources.values);
        background_estimate::interpolate_from_grid(
            tile_grid,
            Planes {
                background: None,
                noise: &mut noise,
            },
            &mut workspace.interpolation,
        );
        Self { noise, floor }
    }

    /// The residual level `sigma` noise σ above the sky at `pos`, with σ held to the floor — the
    /// same test the threshold mask applies to every pixel.
    pub(crate) fn threshold_at(&self, pos: Vec2us, sigma: f32) -> f32 {
        sigma * self.noise[(pos.x, pos.y)].max(self.floor)
    }
}
