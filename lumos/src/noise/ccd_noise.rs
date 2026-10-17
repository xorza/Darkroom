//! [`CcdNoise`]: the CCD equation for one frame's channel or color.

use crate::image::background_split::BackgroundSplit;

/// The noise variance of a sample at value `x` where a flat multiplied it by `g`, in the frame's
/// own units (Merline & Howell 1995, carried through the flat):
///
/// `variance(x, g) = max(background(g), q²·g²) + max(x − sky, 0)·g / electrons_per_unit`
///
/// The background is the white noise measured at the sky level, split by how the flat amplified it
/// ([`BackgroundSplit`]): it already holds the read noise, the dark current, the quantization noise
/// and the sky's own photon noise, so no consumer adds them again. `q` is the source's ADC step σ,
/// which the flat scaled like the read noise. The source term counts the photons above the sky,
/// which arrived through the flat as the sky's did, and only when the gain is known.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct CcdNoise {
    pub(crate) background: BackgroundSplit,
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
            background: self.background_at(gain),
            sky: self.sky,
            per_unit: self
                .electrons_per_unit
                .map_or(0.0, |electrons| gain / electrons),
        }
    }

    /// The variance of the background where a flat multiplied the sample by `gain`.
    const fn background_at(self, gain: f32) -> f32 {
        self.background
            .at(gain)
            .max(self.quantization_variance * gain * gain)
    }
}

/// The CCD equation at one flat gain: `variance(x) = background + max(x − sky, 0)·per_unit`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct NoiseAt {
    /// The background's variance.
    pub(crate) background: f32,
    pub(crate) sky: f32,
    /// The variance a unit of signal above the sky adds: the inverse of the electrons a unit
    /// holds; 0 where the gain is unknown.
    pub(crate) per_unit: f32,
}

impl NoiseAt {
    /// The variance of a sample at value `x`.
    #[inline(always)]
    pub(crate) fn variance(self, x: f32) -> f32 {
        self.background + (x - self.sky).max(0.0) * self.per_unit
    }

    /// The model of a sample that averaged `confidence` source pixels' worth of white noise, as a
    /// warp's interpolation does: both terms over it.
    pub(crate) fn over(self, confidence: f32) -> Self {
        Self {
            background: self.background / confidence,
            per_unit: self.per_unit / confidence,
            ..self
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The background where a flat multiplied the sample by `g`. Background 1/4 at gain 1, half of
    /// it read noise, all dyadic so exact:
    /// - gain 1: 1/4; gain 2: `1/4·2·(½·2 + ½)` = 3/4;
    /// - the split matters: all read noise gives `1/4·4` = 1 at gain 2, all sky `1/4·2` = 1/2;
    /// - a step of σ 1/4 floors a silent background at `(1/4·2)²` = 1/4 at gain 2.
    #[test]
    fn the_background_follows_the_flat_gain() {
        let noise = CcdNoise {
            background: BackgroundSplit {
                variance: 0.25,
                read_share: 0.5,
            },
            quantization_variance: 0.0,
            sky: 1.0,
            electrons_per_unit: Some(4.0),
        };
        assert_eq!(noise.at(1.0).background, 0.25);
        assert_eq!(noise.at(2.0).background, 0.75);
        // Above the sky of 1, a unit at gain 2 adds 2/4: x = 3 lies 2 units up, 3/4 + 2·1/2 = 7/4;
        // below the sky only the background remains. A warp's confidence of 2 halves both.
        assert_eq!(noise.at(2.0).variance(3.0), 1.75);
        assert_eq!(noise.at(2.0).variance(0.5), 0.75);
        assert_eq!(noise.at(2.0).over(2.0).variance(3.0), 0.875);
        let no_gain = CcdNoise {
            electrons_per_unit: None,
            ..noise
        };
        for (read_share, expected) in [(1.0, 1.0), (0.0, 0.5)] {
            let split = CcdNoise {
                background: BackgroundSplit {
                    variance: 0.25,
                    read_share,
                },
                ..no_gain
            };
            assert_eq!(split.at(2.0).background, expected, "ρ {read_share}");
            // With no gain, a sample above the sky adds nothing to the background.
            assert_eq!(split.at(2.0).variance(3.0), expected, "ρ {read_share}");
        }
        let silent = CcdNoise {
            background: BackgroundSplit::unflattened(0.0),
            quantization_variance: 1.0 / 16.0,
            ..no_gain
        };
        assert_eq!(silent.at(2.0).background, 0.25);
    }
}
