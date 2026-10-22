//! [`PixelSamples`]: what a reducer reads at one pixel.

use crate::noise::ccd_noise::NoiseAt;

/// One pixel's gathered samples, as a reducer reads them.
#[derive(Debug)]
pub(crate) struct PixelSamples<'a> {
    pub(crate) values: &'a mut [f32],
    pub(crate) weights: &'a [f32],
    /// Each sample's noise model, when the combine measures a spread or a variance.
    pub(crate) noise: Option<NoiseCols<'a>>,
}

/// The gathered samples' noise models, one column per term, at each sample's flat gain `g` and over
/// a warp's confidence `q`: the background is the model's at `g` over `q`, and a unit above the sky
/// carries `g/(electrons·q)`, since the flat multiplied the sample and an interpolated sample
/// averaged `q` source pixels' worth of white noise.
#[derive(Debug, Clone, Copy)]
pub(crate) struct NoiseCols<'a> {
    pub(crate) bg: &'a [f32],
    pub(crate) sky: &'a [f32],
    /// [`NoiseAt::per_unit`], 0 where the gain is unknown.
    pub(crate) per_unit: &'a [f32],
}

impl NoiseCols<'_> {
    /// The variance of sample `idx` at value `x`.
    #[inline(always)]
    pub(crate) fn variance_at(&self, idx: usize, x: f32) -> f32 {
        NoiseAt {
            bg: self.bg[idx],
            sky: self.sky[idx],
            per_unit: self.per_unit[idx],
        }
        .variance(x)
    }
}
