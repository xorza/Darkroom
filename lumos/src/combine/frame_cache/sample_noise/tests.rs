use arrayvec::ArrayVec;

use crate::combine::frame_cache::normalized_stats::NormalizedStats;
use crate::combine::frame_cache::sample_noise::SampleNoise;
use crate::combine::frame_read::slots::Slots;
use crate::combine::normalization::FrameNorm;
use crate::combine::normalization::SlotNorm;
use crate::combine::reducer::pixel_samples::NoiseColumns;
use crate::combine::rejection::pass::SampleBand;
use crate::combine::rejection::sigma_bounds::SigmaBounds;
use crate::frame::frame_stats::FrameStats;
use crate::image::background_split::BackgroundSplit;
use crate::image::cfa::cfa_pattern::CfaPattern;
use crate::image::cfa::cfa_type::CfaType;
use crate::math::vec2us::Vec2us;
use crate::noise::ccd_noise::CcdNoise;

fn stats(noise: &[f32], quantization_sigma: Option<f32>, electrons: Option<f32>) -> FrameStats {
    FrameStats {
        medians: [0.5].into_iter().collect(),
        background: noise
            .iter()
            .copied()
            .map(BackgroundSplit::of_sigma)
            .collect(),
        sky: noise.iter().map(|_| 0.5).collect(),
        spread: None,
        quantization_sigma,
        electrons_per_unit: electrons,
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
/// - Frame 1 without a gain: its quantization σ 1/2 raises the 1/4 to 1/2, and it is the first
///   that states none.
/// - An RGGB mosaic takes the color of the pixel, each with its color's norm: gains 2, 2, 1/2
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
    assert_eq!(noise.model(1, 0).at(1.0).background, 0.25);
    assert_eq!(noise.model(1, 2).at(1.0).background, 1.0);
    assert_eq!(noise.first_without_gain(), Some(1));

    let mosaic_slots = Slots::new(Some(CfaType::Bayer(CfaPattern::Rggb)), 1);
    let mosaic = SampleNoise::new(NormalizedStats {
        stats: &[stats(&[0.25, 0.5, 1.0], None, Some(8.0))],
        norms: Some(&[norms(&[2.0, 2.0, 0.5], 0.125)]),
        slots: mosaic_slots,
    });
    for (x, y, color, variance, sky) in [
        (0, 0, 0, 0.25, 1.125),
        (1, 0, 1, 1.0, 1.125),
        (0, 1, 1, 1.0, 1.125),
        (1, 1, 2, 0.25, 0.375),
    ] {
        let slot = mosaic_slots.slot(0, Vec2us::new(x, y));
        assert_eq!(slot, color, "({x}, {y})");
        assert_eq!(
            mosaic.model(0, slot).at(1.0).background,
            variance,
            "({x}, {y})"
        );
        assert_eq!(mosaic.model(0, slot).sky, sky, "({x}, {y})");
    }
    assert_eq!(mosaic.first_without_gain(), None);
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
        per_unit: &[0.25, 0.0],
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
