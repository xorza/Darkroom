//! [`SkyNoise`]: what detection keeps of the sky once it is subtracted.

use imaginarium::Buffer2;

use crate::math::vec2us::Vec2us;
use crate::stacking::star_detection::resources::DetectionResources;

/// The sky's noise per pixel and its floor: all the stages after the sky's removal need of it.
///
/// The sky level itself went into the residual plane `pixels − background` that those stages read,
/// so none of them can see sky-included values — the threshold, the labeling, both deblenders and
/// the measurement all work on the source alone.
#[derive(Debug)]
pub(crate) struct SkyNoise {
    /// Per-pixel noise (σ).
    pub(crate) noise: Buffer2<f32>,
    /// The floor every threshold built from [`Self::noise`] applies to it — see
    /// `background_estimate::noise_floor_from`.
    pub(crate) floor: f32,
}

impl SkyNoise {
    /// The residual level `sigma` noise σ above the sky at `pos`, with σ held to the floor — the
    /// same test the threshold mask applies to every pixel.
    pub(crate) fn threshold_at(&self, pos: Vec2us, sigma: f32) -> f32 {
        sigma * self.noise[(pos.x, pos.y)].max(self.floor)
    }

    pub(crate) fn release_to_pool(self, pool: &mut DetectionResources) {
        pool.release_f32(self.noise);
    }
}
