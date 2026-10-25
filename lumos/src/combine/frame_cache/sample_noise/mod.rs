//! [`SampleNoise`]: each frame's noise model as the combine sees it, and the per-sample columns the
//! gather fills from it.

use crate::combine::frame_cache::normalized_stats::NormalizedStats;
use crate::combine::frame_read::slots::Slots;
use crate::frame::frame_stats::FrameStats;
use crate::image::metadata::bg_split::BgSplit;
use crate::noise::ccd_noise::CcdNoise;

/// Each frame's [`CcdNoise`] per slot, carried through its normalization into the units the frames
/// are combined in.
#[derive(Debug)]
pub(crate) struct SampleNoise {
    slots: Slots,
    /// Frame-major, `slots.count()` per frame.
    models: Vec<CcdNoise>,
    /// The first frame that states no gain, whose model has no source term.
    first_without_gain: Option<usize>,
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
        let mut first_without_gain = None;
        for (frame, stats) in stats.into_iter().enumerate() {
            debug_assert_eq!(stats.bg.len(), slots.count());
            if stats.electrons_per_unit.is_none() {
                first_without_gain.get_or_insert(frame);
            }
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
            first_without_gain,
        }
    }

    /// The model of `gain·x + offset`: the background and step variances scale by `gain²`, the sky
    /// maps as a sample does, and a unit of the result holds `electrons/gain` electrons.
    fn normalized(model: CcdNoise, gain: f32, offset: f32) -> CcdNoise {
        debug_assert!(gain > 0.0, "a normalization gain is positive, not {gain}");
        CcdNoise {
            bg: BgSplit {
                variance: model.bg.variance * gain * gain,
                ..model.bg
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

    /// The first frame that states no gain; `None` when every frame states one.
    pub(crate) const fn first_without_gain(&self) -> Option<usize> {
        self.first_without_gain
    }
}

#[cfg(test)]
mod tests;
