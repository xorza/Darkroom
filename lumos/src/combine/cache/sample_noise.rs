//! [`SampleNoise`]: each frame's noise as the combine sees it.

use crate::combine::normalization::FrameNorm;
use crate::frame_store::frame_stats::FrameStats;
use crate::io::image::cfa::CfaType;
use crate::math::vec2us::Vec2us;

/// Each frame's noise variance after normalization, per channel, or per colour of a mosaic.
///
/// The σ is the white noise [`FrameStats::noise`] measured, raised to the frame's quantization σ:
/// on integer data with less noise than one step the measurement can read 0, and no sample is known
/// better than its step. Normalization scales a frame by its gain, so the variance is `(gain·σ)²`.
#[derive(Debug)]
pub(crate) struct SampleNoise {
    /// Frame-major, `slots` per frame.
    variances: Vec<f32>,
    slots: usize,
    /// The pattern whose colours the slots are, for a mosaic. `None` makes the slots channels.
    mosaic: Option<CfaType>,
}

impl SampleNoise {
    pub(crate) fn new<'a>(
        stats: impl IntoIterator<Item = &'a FrameStats>,
        frame_norms: Option<&[FrameNorm]>,
        cfa_type: Option<CfaType>,
    ) -> Self {
        let mosaic = cfa_type.filter(|cfa| matches!(cfa, CfaType::Bayer(_) | CfaType::XTrans(_)));
        let mut variances = Vec::new();
        let mut slots = 0;
        for (frame, stats) in stats.into_iter().enumerate() {
            debug_assert!(slots == 0 || stats.noise.len() == slots);
            slots = stats.noise.len();
            let floor = stats.quantization_sigma.unwrap_or(0.0);
            variances.extend(stats.noise.iter().enumerate().map(|(slot, &sigma)| {
                let channel = if mosaic.is_some() { 0 } else { slot };
                let gain = frame_norms.map_or(1.0, |norms| norms[frame].channels[channel].gain);
                (gain * sigma.max(floor)).powi(2)
            }));
        }
        Self {
            variances,
            slots,
            mosaic,
        }
    }

    /// The slot of a pixel of `channel`: its colour on a mosaic, the channel otherwise.
    pub(crate) const fn slot(&self, channel: usize, position: Vec2us) -> usize {
        match &self.mosaic {
            Some(cfa) => cfa.color_at(position) as usize,
            None => channel,
        }
    }

    pub(crate) fn variance(&self, frame: usize, slot: usize) -> f32 {
        debug_assert!(slot < self.slots);
        self.variances[frame * self.slots + slot]
    }
}

#[cfg(test)]
mod tests {
    use arrayvec::ArrayVec;

    use super::*;
    use crate::combine::normalization::ChannelNorm;
    use crate::frame_store::frame_facts::FrameFacts;
    use crate::io::raw::demosaic::bayer::CfaPattern;
    use crate::math::statistics::MedianMad;

    fn stats(noise: &[f32], quantization_sigma: Option<f32>) -> FrameStats {
        FrameStats {
            channels: [MedianMad {
                median: 0.5,
                mad: 0.1,
            }]
            .into_iter()
            .collect(),
            noise: noise.iter().copied().collect(),
            quantization_sigma,
            facts: FrameFacts {
                domain: None,
                row_order: None,
                cfa_type: None,
                saturation_flagged: false,
            },
        }
    }

    /// Variances are `(gain·max(σ, quantization σ))²`, all dyadic so exact.
    /// - RGB, frame 0 with gains 2, 1, 1/2 on σ 1/4, 1/2, 1: 1/4, 1/4, 1/4. Frame 1 without
    ///   normalization reads its own σ², and its quantization σ 1/2 raises the 1/4 to 1/2.
    /// - An RGGB mosaic takes the colour of the pixel: (0, 0) red, (1, 0) and (0, 1) green, (1, 1)
    ///   blue, each with the gain of its one channel.
    #[test]
    fn variances_follow_the_gain_and_the_quantization_floor() {
        let norms = |gains: &[f32]| FrameNorm {
            channels: gains
                .iter()
                .map(|&gain| ChannelNorm { gain, offset: 0.0 })
                .collect::<ArrayVec<_, 3>>(),
        };
        let frames = [
            stats(&[0.25, 0.5, 1.0], None),
            stats(&[0.25, 0.5, 1.0], Some(0.5)),
        ];
        let noise = SampleNoise::new(
            &frames,
            Some(&[norms(&[2.0, 1.0, 0.5]), norms(&[1.0; 3])]),
            None,
        );
        let origin = Vec2us::new(0, 0);
        for channel in 0..3 {
            assert_eq!(noise.slot(channel, origin), channel);
            assert_eq!(noise.variance(0, channel), 0.25);
        }
        assert_eq!(noise.variance(1, 0), 0.25);
        assert_eq!(noise.variance(1, 1), 0.25);
        assert_eq!(noise.variance(1, 2), 1.0);

        let mosaic = SampleNoise::new(
            &[stats(&[0.25, 0.5, 1.0], None)],
            Some(&[norms(&[2.0])]),
            Some(CfaType::Bayer(CfaPattern::Rggb)),
        );
        for (x, y, colour, variance) in [
            (0, 0, 0, 0.25),
            (1, 0, 1, 1.0),
            (0, 1, 1, 1.0),
            (1, 1, 2, 4.0),
        ] {
            let slot = mosaic.slot(0, Vec2us::new(x, y));
            assert_eq!(slot, colour, "({x}, {y})");
            assert_eq!(mosaic.variance(0, slot), variance, "({x}, {y})");
        }
    }
}
