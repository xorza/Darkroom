use arrayvec::ArrayVec;

use crate::combine::normalization::{FrameNorm, SlotNorm};
use crate::combine::stack::quantization;
use crate::frame::frame_stats::FrameStats;
use crate::image::bg_split::BgSplit;
use crate::math::statistics;

fn stats(quantization_sigma: Option<f32>) -> FrameStats {
    FrameStats {
        medians: [0.5].into_iter().collect(),
        bg: [statistics::internals::mad_to_sigma(0.1)]
            .into_iter()
            .map(BgSplit::of_sigma)
            .collect(),
        sky: [0.5].into_iter().collect(),
        spread: None,
        quantization_sigma,
        electrons_per_unit: None,
    }
}

fn norms(gains: &[f32]) -> Vec<FrameNorm> {
    gains
        .iter()
        .map(|&gain| {
            let mut slots = ArrayVec::new();
            slots.push(SlotNorm { gain, offset: 0.0 });
            FrameNorm { slots }
        })
        .collect()
}

/// The largest step, scaled by each frame's gains, every one dyadic so exact:
/// - unnormalized, σ 1/4 and 1/2 give 1/2;
/// - gains −2 and 1 on σ 1/4 give 1/2, by magnitude;
/// - two channels of gains 1 and 2 on σ 1/4 give channel 1's 1/2, where channel 0 alone would
///   give 1/4;
/// - a set with one frame of no finite, positive σ states none.
#[test]
fn a_master_states_the_largest_scaled_step() {
    let largest = |sigmas: &[Option<f32>], norms: Option<&[FrameNorm]>| {
        let stats: Vec<FrameStats> = sigmas.iter().map(|&sigma| stats(sigma)).collect();
        quantization::largest_step(&stats, norms)
    };
    assert_eq!(largest(&[Some(0.25), Some(0.5)], None), Some(0.5));
    let quarter = [Some(0.25); 2];
    assert_eq!(largest(&quarter, None), Some(0.25));
    assert_eq!(largest(&quarter, Some(&norms(&[-2.0, 1.0]))), Some(0.5));
    let two_channels: Vec<FrameNorm> = (0..2)
        .map(|_| FrameNorm {
            slots: [1.0, 2.0]
                .into_iter()
                .map(|gain| SlotNorm { gain, offset: 0.0 })
                .collect(),
        })
        .collect();
    assert_eq!(largest(&quarter, Some(&two_channels)), Some(0.5));
    for unusable in [
        None,
        Some(0.0),
        Some(-0.25),
        Some(f32::NAN),
        Some(f32::INFINITY),
    ] {
        assert_eq!(largest(&[Some(0.25), unusable], None), None, "{unusable:?}");
    }
}
