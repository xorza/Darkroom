//! [`SampleNoise`]: each frame's noise model as the combine sees it, and the per-sample columns the
//! gather fills from it.

use crate::combine::frame_cache::normalized_stats::NormalizedStats;
use crate::combine::frame_cache::slots::Slots;
use crate::frame::frame_stats::FrameStats;
use crate::math::noise::background_split::BackgroundSplit;
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
    /// The models of `frames`, each carried through its norm.
    pub(crate) fn new<'a>(
        frames: NormalizedStats<'a, impl IntoIterator<Item = &'a FrameStats>>,
    ) -> Self {
        let NormalizedStats {
            stats,
            norms: frame_norms,
            slots,
        } = frames;
        let mut models = Vec::new();
        let mut every_gain_known = true;
        for (frame, stats) in stats.into_iter().enumerate() {
            debug_assert_eq!(stats.noise.len(), slots.count());
            every_gain_known &= stats.electrons_per_unit.is_some();
            models.extend((0..slots.count()).map(|slot| {
                let model = stats.ccd_noise(slot);
                match frame_norms {
                    Some(norms) => {
                        let norm = norms[frame].slots[slot];
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

    /// The model of `gain·x + offset`: the background and step variances scale by `gain²`, the sky
    /// maps as a sample does, and a unit of the result holds `electrons/gain` electrons.
    fn normalized(model: CcdNoise, gain: f32, offset: f32) -> CcdNoise {
        debug_assert!(gain > 0.0, "a normalization gain is positive, not {gain}");
        CcdNoise {
            background: BackgroundSplit {
                variance: model.background.variance * gain * gain,
                ..model.background
            },
            quantization_variance: model.quantization_variance * gain * gain,
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

/// The gathered samples' noise models, one column per term, at each sample's flat gain `g` and over
/// a warp's confidence `q`: the background is the model's at `g` over `q`, and a unit above the sky
/// carries `g/(electrons·q)`, since the flat multiplied the sample and an interpolated sample
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
}

#[cfg(test)]
mod tests;
