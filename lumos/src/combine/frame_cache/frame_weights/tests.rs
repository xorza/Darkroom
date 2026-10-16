use arrayvec::ArrayVec;

use crate::combine::error::StackError;
use crate::combine::frame_cache::frame_weights::FrameWeights;
use crate::combine::frame_cache::normalized_stats::NormalizedStats;
use crate::combine::frame_read::slots::Slots;
use crate::combine::normalization::{FrameNorm, SlotNorm};
use crate::combine::stack_config::weighting::Weighting;
use crate::frame::frame_stats::FrameStats;
use crate::image::background_split::BackgroundSplit;
use crate::stack_product::weight_kind::WeightKind;

fn stats(noise: &[f32]) -> FrameStats {
    FrameStats {
        medians: noise.iter().map(|_| 0.5).collect(),
        background: noise
            .iter()
            .copied()
            .map(BackgroundSplit::of_sigma)
            .collect(),
        sky: noise.iter().map(|_| 0.5).collect(),
        spread: None,
        quantization_sigma: None,
        electrons_per_unit: None,
    }
}

fn norm(gain: f32) -> FrameNorm {
    FrameNorm {
        slots: [SlotNorm { gain, offset: 0.0 }]
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
        NormalizedStats {
            stats: &[stats(&[1.0 / 64.0]), stats(&[1.0 / 64.0])],
            norms: Some(&[norm(1.0), norm(2.0)]),
            slots: mono,
        },
    )
    .unwrap()
    .unwrap();
    assert_eq!(
        (weights.weight(0, 0), weights.weight(1, 0)),
        (4096.0, 1024.0)
    );
    assert_eq!(weights.kind(), WeightKind::InverseVariance);

    let rgb = Slots::new(None, 3);
    let weights = FrameWeights::resolve(
        &Weighting::Noise,
        NormalizedStats {
            stats: &[stats(&[1.0 / 64.0, 1.0 / 64.0, 1.0 / 16.0])],
            norms: None,
            slots: rgb,
        },
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
    let manual = FrameWeights::resolve(
        &Weighting::Manual(vec![1.0, 3.0]),
        NormalizedStats {
            stats: &[],
            norms: None,
            slots: rgb,
        },
    )
    .unwrap()
    .unwrap();
    assert_eq!([0, 1, 2].map(|slot| manual.weight(1, slot)), [3.0; 3]);
    assert_eq!(manual.weight(0, 2), 1.0);
    assert_eq!(manual.kind(), WeightKind::Relative);
    assert!(
        FrameWeights::resolve(
            &Weighting::Equal,
            NormalizedStats {
                stats: &[],
                norms: None,
                slots: rgb
            },
        )
        .unwrap()
        .is_none()
    );
    let noiseless = FrameWeights::resolve(
        &Weighting::Noise,
        NormalizedStats {
            stats: &[stats(&[0.25]), stats(&[0.0])],
            norms: None,
            slots: Slots::new(None, 1),
        },
    );
    assert!(matches!(
        noiseless,
        Err(StackError::NoNoiseToWeigh { index: 1 })
    ));
}
