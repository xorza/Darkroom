//! [`FrameWeights`]: each frame's weight in the mean, per slot.

use std::iter;

use crate::combine::error::StackError;
use crate::combine::frame_cache::normalized_stats::NormalizedStats;
use crate::combine::frame_cache::slots::Slots;
use crate::combine::stack_config::Weighting;
use crate::frame::frame_stats::FrameStats;

/// Each frame's weight per slot, not normalized.
///
/// `Weighting::Noise` gives `1 / (gain·σ)²`, σ the frame's measured background noise in that slot
/// and `gain` its normalization: the inverse variance of the frame as combined. So where no warp
/// averaged and no flat amplified the samples the weight plane, `Σwᵢ` over the survivors, is the
/// inverse variance of their mean. Per slot, because a frame with a bad blue channel is noisier in
/// blue only. Manual weights are relative and the same in every slot.
#[derive(Debug)]
pub(crate) struct FrameWeights {
    slots: Slots,
    /// Frame-major, `slots.count()` per frame.
    values: Vec<f32>,
}

impl FrameWeights {
    /// The weights `weighting` asks for of `frames`, or `None` for equal ones.
    ///
    /// # Errors
    /// [`StackError::NoNoiseToWeigh`] when noise weighting meets a frame with no measured noise in
    /// a slot.
    pub(crate) fn resolve<'a>(
        weighting: &Weighting,
        frames: NormalizedStats<'a, impl IntoIterator<Item = &'a FrameStats>>,
    ) -> Result<Option<Self>, StackError> {
        let NormalizedStats {
            stats,
            norms: frame_norms,
            slots,
        } = frames;
        let values = match weighting {
            Weighting::Equal => return Ok(None),
            Weighting::Manual(weights) => weights
                .iter()
                .flat_map(|&weight| iter::repeat_n(weight, slots.count()))
                .collect(),
            Weighting::Noise => {
                let mut values = Vec::new();
                for (index, stats) in stats.into_iter().enumerate() {
                    for slot in 0..slots.count() {
                        let gain = frame_norms.map_or(1.0, |norms| norms[index].slots[slot].gain);
                        // The frame's noise where no flat amplified it, as normalization scaled
                        // it: the flat's gain changes across the frame, not between frames.
                        let variance = stats.ccd_noise(slot).background_at(1.0) * gain * gain;
                        let weight = 1.0 / variance;
                        if !weight.is_finite() {
                            return Err(StackError::NoNoiseToWeigh { index });
                        }
                        values.push(weight);
                    }
                }
                values
            }
        };
        Ok(Some(Self { slots, values }))
    }

    pub(crate) fn weight(&self, frame: usize, slot: usize) -> f32 {
        debug_assert!(slot < self.slots.count());
        self.values[frame * self.slots.count() + slot]
    }
}

#[cfg(test)]
mod tests;
