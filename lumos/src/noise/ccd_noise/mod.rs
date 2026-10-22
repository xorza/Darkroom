//! [`CcdNoise`]: the CCD equation for one frame's channel or color.

use crate::image::bg_split::BgSplit;

/// The noise variance of a sample at value `x` where a flat multiplied it by `g`, in the frame's
/// own units (Merline & Howell 1995, carried through the flat):
///
/// `variance(x, g) = max(bg(g), q²·g²) + max(x − sky, 0)·g / electrons_per_unit`
///
/// The background is the white noise measured at the sky level, split by how the flat amplified it
/// ([`BgSplit`]): it already holds the read noise, the dark current, the quantization noise
/// and the sky's own photon noise, so no consumer adds them again. `q` is the source's ADC step σ,
/// which the flat scaled like the read noise. The source term counts the photons above the sky,
/// which arrived through the flat as the sky's did, and only when the gain is known.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct CcdNoise {
    pub(crate) bg: BgSplit,
    /// `q²`, 0 where the source states no step.
    pub(crate) quantization_variance: f32,
    pub(crate) sky: f32,
    pub(crate) electrons_per_unit: Option<f32>,
}

impl CcdNoise {
    /// The model where a flat multiplied the samples by `gain`: the background there, and what a
    /// unit above the sky adds, `g/electrons`, the flat having multiplied the photons' count.
    pub(crate) fn at(self, gain: f32) -> NoiseAt {
        NoiseAt {
            bg: self.bg_at(gain),
            sky: self.sky,
            per_unit: self
                .electrons_per_unit
                .map_or(0.0, |electrons| gain / electrons),
        }
    }

    /// The variance of the background where a flat multiplied the sample by `gain`.
    const fn bg_at(self, gain: f32) -> f32 {
        self.bg
            .at(gain)
            .max(self.quantization_variance * gain * gain)
    }
}

/// The CCD equation at one flat gain: `variance(x) = bg + max(x − sky, 0)·per_unit`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct NoiseAt {
    /// The background's variance.
    pub(crate) bg: f32,
    pub(crate) sky: f32,
    /// The variance a unit of signal above the sky adds: the inverse of the electrons a unit
    /// holds; 0 where the gain is unknown.
    pub(crate) per_unit: f32,
}

impl NoiseAt {
    /// The variance of a sample at value `x`.
    #[inline(always)]
    pub(crate) fn variance(self, x: f32) -> f32 {
        self.bg + (x - self.sky).max(0.0) * self.per_unit
    }

    /// The model of a sample that averaged `confidence` source pixels' worth of white noise, as a
    /// warp's interpolation does: both terms over it.
    pub(crate) fn over(self, confidence: f32) -> Self {
        Self {
            bg: self.bg / confidence,
            per_unit: self.per_unit / confidence,
            ..self
        }
    }
}

#[cfg(test)]
mod tests;
