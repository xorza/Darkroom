use arrayvec::ArrayVec;

use crate::combine::normalization::{FrameNorm, SlotNorm};
use crate::combine::stack::quantization::SourceSigmas;
use crate::frame::capture_conditions::CaptureConditions;
use crate::frame::frame_facts::FrameFacts;
use crate::frame::frame_stats::FrameStats;
use crate::frame::unverified_conditions::UnverifiedConditions;
use crate::math::statistics::mad_to_sigma;

fn stats(quantization_sigma: Option<f32>) -> FrameStats {
    FrameStats {
        medians: [0.5].into_iter().collect(),
        noise: [mad_to_sigma(0.1)].into_iter().collect(),
        read_share: [0.0; 3].into_iter().collect(),
        sky: [0.5].into_iter().collect(),
        spread: None,
        quantization_sigma,
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

/// A set has sigmas to propagate only when every frame declares a finite, positive one.
#[test]
fn source_sigmas_need_every_frame_to_declare_one() {
    let measure = |sigmas: &[Option<f32>]| {
        SourceSigmas::measure(&sigmas.iter().map(|&sigma| stats(sigma)).collect::<Vec<_>>())
            .map(|sigmas| sigmas.0)
    };
    assert_eq!(measure(&[Some(0.25), Some(0.5)]), Some(vec![0.25, 0.5]));
    for unusable in [
        None,
        Some(0.0),
        Some(-0.25),
        Some(f32::NAN),
        Some(f32::INFINITY),
    ] {
        assert_eq!(measure(&[Some(0.25), unusable]), None, "{unusable:?}");
    }
}

/// The largest step, scaled by each frame's gains, every one dyadic so exact:
/// - unnormalized, σ 1/4 and 1/2 give 1/2;
/// - gains −2 and 1 on σ 1/4 give 1/2, by magnitude;
/// - two channels of gains 1 and 2 on σ 1/4 give channel 1's 1/2, where channel 0 alone would
///   give 1/4.
#[test]
fn a_master_states_the_largest_scaled_step() {
    assert_eq!(SourceSigmas(vec![0.25, 0.5]).largest(None), Some(0.5));
    let quarter = SourceSigmas(vec![0.25; 2]);
    assert_eq!(quarter.largest(None), Some(0.25));
    assert_eq!(quarter.largest(Some(&norms(&[-2.0, 1.0]))), Some(0.5));
    let two_channels: Vec<FrameNorm> = (0..2)
        .map(|_| FrameNorm {
            slots: [1.0, 2.0]
                .into_iter()
                .map(|gain| SlotNorm { gain, offset: 0.0 })
                .collect(),
        })
        .collect();
    assert_eq!(quarter.largest(Some(&two_channels)), Some(0.5));
}
