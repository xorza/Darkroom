//! [`SampleNoise`]: each frame's noise model as the combine sees it, and the per-sample columns the
//! gather fills from it.

use crate::combine::cache::slots::Slots;
use crate::combine::normalization::FrameNorm;
use crate::frame_store::frame_stats::FrameStats;
use crate::math::noise::ccd_noise::CcdNoise;

/// Each frame's [`CcdNoise`] per slot, carried through its normalization into the units the frames
/// are combined in.
#[derive(Debug)]
pub(crate) struct SampleNoise {
    slots: Slots,
    /// Frame-major, `slots.count()` per frame.
    models: Vec<CcdNoise>,
    /// Whether every frame states its gain, so that every model has its source term.
    every_gain_known: bool,
}

impl SampleNoise {
    pub(crate) fn new<'a>(
        stats: impl IntoIterator<Item = &'a FrameStats>,
        frame_norms: Option<&[FrameNorm]>,
        slots: Slots,
    ) -> Self {
        let mut models = Vec::new();
        let mut every_gain_known = true;
        for (frame, stats) in stats.into_iter().enumerate() {
            debug_assert_eq!(stats.noise.len(), slots.count());
            every_gain_known &= stats.electrons_per_unit.is_some();
            models.extend((0..slots.count()).map(|slot| {
                let model = stats.ccd_noise(slot);
                match frame_norms {
                    Some(norms) => {
                        let norm = norms[frame].channels[slots.channel(slot)];
                        Self::normalized(model, norm.gain, norm.offset)
                    }
                    None => model,
                }
            }));
        }
        Self {
            slots,
            models,
            every_gain_known,
        }
    }

    /// The model of `gain·x + offset`: the background variance scales by `gain²`, the sky maps as
    /// a sample does, and a unit of the result holds `electrons/gain` electrons.
    fn normalized(model: CcdNoise, gain: f32, offset: f32) -> CcdNoise {
        debug_assert!(gain > 0.0, "a normalization gain is positive, not {gain}");
        CcdNoise {
            background_variance: model.background_variance * gain * gain,
            sky: model.sky * gain + offset,
            electrons_per_unit: model.electrons_per_unit.map(|electrons| electrons / gain),
        }
    }

    pub(crate) const fn slots(&self) -> Slots {
        self.slots
    }

    pub(crate) fn model(&self, frame: usize, slot: usize) -> CcdNoise {
        debug_assert!(slot < self.slots.count());
        self.models[frame * self.slots.count() + slot]
    }

    pub(crate) const fn every_gain_known(&self) -> bool {
        self.every_gain_known
    }
}

/// The gathered samples' noise models, one column per term, over a warp's confidence `q`: the
/// variances divide by `q` and the electrons per unit multiply by it, since an interpolated sample
/// averaged `q` source pixels' worth of white noise.
#[derive(Debug, Clone, Copy)]
pub(crate) struct NoiseColumns<'a> {
    pub(crate) background: &'a [f32],
    pub(crate) sky: &'a [f32],
    /// `1 / electrons_per_unit`, and 0 where the gain is unknown.
    pub(crate) inverse_electrons: &'a [f32],
}

impl NoiseColumns<'_> {
    /// The variance of sample `index` at value `x`.
    pub(crate) fn variance_at(&self, index: usize, x: f32) -> f32 {
        self.background[index] + (x - self.sky[index]).max(0.0) * self.inverse_electrons[index]
    }

    /// The root mean square of the samples' background σ: the floor under a measured spread.
    pub(crate) fn background_rms(&self) -> f32 {
        (self.background.iter().sum::<f32>() / self.background.len() as f32).sqrt()
    }
}

#[cfg(test)]
mod tests {
    use arrayvec::ArrayVec;

    use super::*;
    use crate::combine::normalization::ChannelNorm;
    use crate::frame_store::capture_conditions::CaptureConditions;
    use crate::frame_store::frame_facts::FrameFacts;
    use crate::io::image::cfa::CfaType;
    use crate::io::image::unverified_conditions::UnverifiedConditions;
    use crate::io::raw::demosaic::bayer::CfaPattern;
    use crate::math::statistics::MedianMad;
    use crate::math::vec2us::Vec2us;

    fn stats(noise: &[f32], quantization_sigma: Option<f32>, electrons: Option<f32>) -> FrameStats {
        FrameStats {
            channels: [MedianMad {
                median: 0.5,
                mad: 0.1,
            }]
            .into_iter()
            .collect(),
            noise: noise.iter().copied().collect(),
            sky: noise.iter().map(|_| 0.5).collect(),
            quantization_sigma,
            electrons_per_unit: electrons,
            facts: FrameFacts {
                domain: None,
                row_order: None,
                cfa_type: None,
                saturation_flagged: false,
                conditions: CaptureConditions::default(),
                unverified_dark: UnverifiedConditions::NONE,
            },
        }
    }

    fn norms(gains: &[f32], offset: f32) -> FrameNorm {
        FrameNorm {
            channels: gains
                .iter()
                .map(|&gain| ChannelNorm { gain, offset })
                .collect::<ArrayVec<_, 3>>(),
        }
    }

    /// The models follow the gain, the offset and the quantization floor, all dyadic so exact.
    /// - RGB, frame 0 with gains 2, 1, 1/2 on σ 1/4, 1/2, 1: background 1/4 in every channel; the
    ///   sky 0.5 maps to 1, 0.5 and 0.25 (offset 0); 8 electrons per unit become 4, 8 and 16.
    /// - Frame 1 without a gain: its quantization σ 1/2 raises the 1/4 to 1/2, and not every gain
    ///   is known.
    /// - An RGGB mosaic takes the colour of the pixel, each with the gain of its one channel.
    #[test]
    fn models_follow_the_normalization_and_the_quantization_floor() {
        let frames = [
            stats(&[0.25, 0.5, 1.0], None, Some(8.0)),
            stats(&[0.25, 0.5, 1.0], Some(0.5), None),
        ];
        let slots = Slots::new(None, 3);
        let noise = SampleNoise::new(
            &frames,
            Some(&[norms(&[2.0, 1.0, 0.5], 0.0), norms(&[1.0; 3], 0.0)]),
            slots,
        );
        for (slot, (sky, electrons)) in [(1.0, 4.0), (0.5, 8.0), (0.25, 16.0)]
            .into_iter()
            .enumerate()
        {
            assert_eq!(slots.slot(slot, Vec2us::new(0, 0)), slot);
            assert_eq!(
                noise.model(0, slot),
                CcdNoise {
                    background_variance: 0.25,
                    sky,
                    electrons_per_unit: Some(electrons)
                }
            );
        }
        assert_eq!(noise.model(1, 0).background_variance, 0.25);
        assert_eq!(noise.model(1, 2).background_variance, 1.0);
        assert!(!noise.every_gain_known());

        let mosaic_slots = Slots::new(Some(CfaType::Bayer(CfaPattern::Rggb)), 1);
        let mosaic = SampleNoise::new(
            &[stats(&[0.25, 0.5, 1.0], None, Some(8.0))],
            Some(&[norms(&[2.0], 0.125)]),
            mosaic_slots,
        );
        for (x, y, colour, variance) in [
            (0, 0, 0, 0.25),
            (1, 0, 1, 1.0),
            (0, 1, 1, 1.0),
            (1, 1, 2, 4.0),
        ] {
            let slot = mosaic_slots.slot(0, Vec2us::new(x, y));
            assert_eq!(slot, colour, "({x}, {y})");
            assert_eq!(
                mosaic.model(0, slot).background_variance,
                variance,
                "({x}, {y})"
            );
            assert_eq!(mosaic.model(0, slot).sky, 1.125);
        }
        assert!(mosaic.every_gain_known());
    }

    /// A column's variance at `x`: background 0.25, sky 1, 1/4 per unit above it. The background
    /// RMS of variances 0.25 and 0.75 is √0.5.
    #[test]
    fn columns_evaluate_the_model_per_sample() {
        let columns = NoiseColumns {
            background: &[0.25, 0.75],
            sky: &[1.0, 1.0],
            inverse_electrons: &[0.25, 0.0],
        };
        assert_eq!(columns.variance_at(0, 3.0), 0.75);
        assert_eq!(columns.variance_at(0, 0.0), 0.25);
        assert_eq!(columns.variance_at(1, 3.0), 0.75);
        assert_eq!(columns.background_rms(), 0.5f32.sqrt());
    }
}
