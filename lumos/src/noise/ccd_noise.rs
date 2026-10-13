//! [`CcdNoise`]: the CCD equation for one frame's channel or colour.

use crate::noise::background_split::BackgroundSplit;

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
    /// The variance of the background where a flat multiplied the sample by `gain`.
    pub(crate) const fn background_at(self, gain: f32) -> f32 {
        self.background
            .at(gain)
            .max(self.quantization_variance * gain * gain)
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
        assert_eq!(noise.background_at(1.0), 0.25);
        assert_eq!(noise.background_at(2.0), 0.75);
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
            assert_eq!(split.background_at(2.0), expected, "ρ {read_share}");
        }
        let silent = CcdNoise {
            background: BackgroundSplit::unflattened(0.0),
            quantization_variance: 1.0 / 16.0,
            ..no_gain
        };
        assert_eq!(silent.background_at(2.0), 0.25);
    }
}
