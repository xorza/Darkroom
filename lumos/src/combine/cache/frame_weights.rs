//! [`FrameWeights`]: each frame's weight in the mean, per slot.

use std::iter;

use crate::combine::cache::slots::Slots;
use crate::combine::config::Weighting;
use crate::combine::error::StackError;
use crate::combine::normalization::FrameNorm;
use crate::frame_store::frame_stats::FrameStats;

/// Each frame's weight per slot, not normalized.
///
/// `Weighting::Noise` gives `1 / (gain·σ)²`, σ the frame's measured background noise in that slot
/// and `gain` its normalization: the inverse variance of the frame as combined. So with unit
/// confidence the weight plane, `Σwᵢ` over the survivors, is the inverse variance of their mean.
/// Per slot, because a frame with a bad blue channel is noisier in blue only. Manual weights are
/// relative and the same in every slot.
#[derive(Debug)]
pub(crate) struct FrameWeights {
    slots: Slots,
    /// Frame-major, `slots.count()` per frame.
    values: Vec<f32>,
}

impl FrameWeights {
    /// The weights `weighting` asks for, or `None` for equal ones.
    ///
    /// # Errors
    /// [`StackError::NoNoiseToWeigh`] when noise weighting meets a frame with no measured noise in a
    /// slot.
    pub(crate) fn resolve<'a>(
        weighting: &Weighting,
        stats: impl IntoIterator<Item = &'a FrameStats>,
        frame_norms: Option<&[FrameNorm]>,
        slots: Slots,
    ) -> Result<Option<Self>, StackError> {
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
                        let gain = frame_norms
                            .map_or(1.0, |norms| norms[index].channels[slots.channel(slot)].gain);
                        let variance = stats.ccd_noise(slot).background_variance * gain * gain;
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
mod tests {
    use arrayvec::ArrayVec;

    use super::*;
    use crate::combine::normalization::ChannelNorm;
    use crate::frame_store::capture_conditions::CaptureConditions;
    use crate::frame_store::frame_facts::FrameFacts;
    use crate::io::image::unverified_conditions::UnverifiedConditions;
    use crate::math::statistics::MedianMad;

    fn stats(noise: &[f32]) -> FrameStats {
        FrameStats {
            channels: noise
                .iter()
                .map(|_| MedianMad {
                    median: 0.5,
                    mad: 0.1,
                })
                .collect(),
            noise: noise.iter().copied().collect(),
            sky: noise.iter().map(|_| 0.5).collect(),
            quantization_sigma: None,
            electrons_per_unit: None,
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

    fn norm(gain: f32) -> FrameNorm {
        FrameNorm {
            channels: [ChannelNorm { gain, offset: 0.0 }]
                .into_iter()
                .collect::<ArrayVec<_, 3>>(),
        }
    }

    /// Noise weights are inverse variances as combined, per slot and not normalized.
    /// - Two frames of σ 1/64, the second normalized by gain 2: 4096 and 4096/4 = 1024, exact.
    /// - An RGB frame with a bad blue channel, σ 1/64, 1/64, 1/16: 4096 in red and green, 256 in
    ///   blue only.
    #[test]
    fn noise_weights_are_inverse_variances_per_slot() {
        let mono = Slots::new(None, 1);
        let weights = FrameWeights::resolve(
            &Weighting::Noise,
            &[stats(&[1.0 / 64.0]), stats(&[1.0 / 64.0])],
            Some(&[norm(1.0), norm(2.0)]),
            mono,
        )
        .unwrap()
        .unwrap();
        assert_eq!(
            (weights.weight(0, 0), weights.weight(1, 0)),
            (4096.0, 1024.0)
        );

        let rgb = Slots::new(None, 3);
        let weights = FrameWeights::resolve(
            &Weighting::Noise,
            &[stats(&[1.0 / 64.0, 1.0 / 64.0, 1.0 / 16.0])],
            None,
            rgb,
        )
        .unwrap()
        .unwrap();
        assert_eq!(
            [0, 1, 2].map(|slot| weights.weight(0, slot)),
            [4096.0, 4096.0, 256.0]
        );
    }

    /// Manual weights are taken as given in every slot, equal weighting needs no table, and a
    /// frame with no measured noise cannot be noise-weighted.
    #[test]
    fn manual_and_equal_weights_and_a_noiseless_frame() {
        let rgb = Slots::new(None, 3);
        let manual = FrameWeights::resolve(&Weighting::Manual(vec![1.0, 3.0]), &[], None, rgb)
            .unwrap()
            .unwrap();
        assert_eq!([0, 1, 2].map(|slot| manual.weight(1, slot)), [3.0; 3]);
        assert_eq!(manual.weight(0, 2), 1.0);
        assert!(
            FrameWeights::resolve(&Weighting::Equal, &[], None, rgb)
                .unwrap()
                .is_none()
        );
        let noiseless = FrameWeights::resolve(
            &Weighting::Noise,
            &[stats(&[0.25]), stats(&[0.0])],
            None,
            Slots::new(None, 1),
        );
        assert!(matches!(
            noiseless,
            Err(StackError::NoNoiseToWeigh { index: 1 })
        ));
    }
}
