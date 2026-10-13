use arrayvec::ArrayVec;

use crate::combine::frame_cache::normalized_stats::NormalizedStats;
use crate::combine::frame_cache::sample_noise::{NoiseColumns, SampleNoise};
use crate::combine::frame_cache::slots::Slots;
use crate::combine::normalization::FrameNorm;
use crate::combine::normalization::SlotNorm;
use crate::combine::rejection::pass::SampleBand;
use crate::combine::rejection::sigma_bounds::SigmaBounds;
use crate::frame::capture_conditions::CaptureConditions;
use crate::frame::frame_facts::FrameFacts;
use crate::frame::frame_stats::FrameStats;
use crate::frame::unverified_conditions::UnverifiedConditions;
use crate::image::cfa::cfa_pattern::CfaPattern;
use crate::image::cfa::cfa_type::CfaType;
use crate::math::noise::background_split::BackgroundSplit;
use crate::math::noise::ccd_noise::CcdNoise;
use crate::math::vec2us::Vec2us;

fn stats(noise: &[f32], quantization_sigma: Option<f32>, electrons: Option<f32>) -> FrameStats {
    FrameStats {
        medians: [0.5].into_iter().collect(),
        noise: noise.iter().copied().collect(),
        read_share: [0.0; 3].into_iter().collect(),
        sky: noise.iter().map(|_| 0.5).collect(),
        spread: None,
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
        slots: gains
            .iter()
            .map(|&gain| SlotNorm { gain, offset })
            .collect::<ArrayVec<_, 3>>(),
    }
}

/// The models follow the gain, the offset and the quantization floor, all dyadic so exact.
/// - RGB, frame 0 with gains 2, 1, 1/2 on σ 1/4, 1/2, 1: background 1/4 in every channel; the
///   sky 0.5 maps to 1, 0.5 and 0.25 (offset 0); 8 electrons per unit become 4, 8 and 16.
/// - Frame 1 without a gain: its quantization σ 1/2 raises the 1/4 to 1/2, and not every gain
///   is known.
/// - An RGGB mosaic takes the colour of the pixel, each with its colour's norm: gains 2, 2, 1/2
///   and offset 1/8 put σ² 1/16, 1/4, 1 at 1/4, 1 and 1/4, and the sky 1/2 at 9/8, 9/8, 3/8.
#[test]
fn models_follow_the_normalization_and_the_quantization_floor() {
    let frames = [
        stats(&[0.25, 0.5, 1.0], None, Some(8.0)),
        stats(&[0.25, 0.5, 1.0], Some(0.5), None),
    ];
    let slots = Slots::new(None, 3);
    let noise = SampleNoise::new(NormalizedStats {
        stats: &frames,
        norms: Some(&[norms(&[2.0, 1.0, 0.5], 0.0), norms(&[1.0; 3], 0.0)]),
        slots,
    });
    for (slot, (sky, electrons)) in [(1.0, 4.0), (0.5, 8.0), (0.25, 16.0)]
        .into_iter()
        .enumerate()
    {
        assert_eq!(slots.slot(slot, Vec2us::new(0, 0)), slot);
        assert_eq!(
            noise.model(0, slot),
            CcdNoise {
                background: BackgroundSplit::unflattened(0.25),
                quantization_variance: 0.0,
                sky,
                electrons_per_unit: Some(electrons)
            }
        );
    }
    assert_eq!(noise.model(1, 0).background_at(1.0), 0.25);
    assert_eq!(noise.model(1, 2).background_at(1.0), 1.0);
    assert!(!noise.every_gain_known());

    let mosaic_slots = Slots::new(Some(CfaType::Bayer(CfaPattern::Rggb)), 1);
    let mosaic = SampleNoise::new(NormalizedStats {
        stats: &[stats(&[0.25, 0.5, 1.0], None, Some(8.0))],
        norms: Some(&[norms(&[2.0, 2.0, 0.5], 0.125)]),
        slots: mosaic_slots,
    });
    for (x, y, colour, variance, sky) in [
        (0, 0, 0, 0.25, 1.125),
        (1, 0, 1, 1.0, 1.125),
        (0, 1, 1, 1.0, 1.125),
        (1, 1, 2, 0.25, 0.375),
    ] {
        let slot = mosaic_slots.slot(0, Vec2us::new(x, y));
        assert_eq!(slot, colour, "({x}, {y})");
        assert_eq!(
            mosaic.model(0, slot).background_at(1.0),
            variance,
            "({x}, {y})"
        );
        assert_eq!(mosaic.model(0, slot).sky, sky, "({x}, {y})");
    }
    assert!(mosaic.every_gain_known());
}

/// A column's variance at `x`: background 0.25, sky 1, 1/4 per unit above it.
///
/// Each sample's band is in its own σ: at a centre of 1, at the sky, sample 0's σ is
/// √0.25 = 0.5 and sample 1's √0.75 ≈ 0.866, so a band of 2σ reaches 2 and ≈ 2.732. A value of
/// 2.5 is outside sample 0's band and inside sample 1's.
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
    let band = SampleBand {
        centre: 1.0,
        bounds: SigmaBounds::symmetric(2.0),
    };
    assert!(band.keeps(&columns, 0, 2.0));
    assert!(!band.keeps(&columns, 0, 2.5));
    assert!(band.keeps(&columns, 1, 2.5));
    assert!(band.keeps(&columns, 1, -0.5));
    assert!(!band.keeps(&columns, 1, -0.75));
}
