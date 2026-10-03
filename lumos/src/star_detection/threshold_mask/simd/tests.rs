//! Cross-checks that every tier's packed words match the scalar reference exactly.

use crate::internals::simd_check::{DATA_SHAPES, SWEEP_WIDTHS};
use crate::simd::tier::Tier;
use crate::star_detection::threshold_mask::internals::test_params;
use crate::star_detection::threshold_mask::simd::ProcessWords;
use crate::star_detection::threshold_mask::simd::internals::process_words_scalar;

/// The shared sweep plus widths spanning whole 64-pixel words. The kernel takes full words from the
/// row and pads the partial last one, so a word exactly filled, a word and a little, and several
/// words with an odd tail are the boundaries that matter here — the shared list alone never
/// reaches two full words.
fn sweep_widths() -> Vec<usize> {
    SWEEP_WIDTHS
        .iter()
        .copied()
        .chain([65, 127, 128, 130, 193])
        .collect()
}

/// The threshold the kernels compare against, from the level they all compute, so a pixel set to
/// it lands exactly on the boundary rather than near it.
fn threshold_at(with_bg: bool, bg: f32, noise: f32, sigma: f32) -> f32 {
    let level = test_params(sigma).level(noise);
    if with_bg { level + bg } else { level }
}

/// One mode on one tier against the scalar reference, over every shared data shape and width,
/// compared exactly: a packed word is a set of detections.
///
/// The inputs lean on the shared shapes for coverage, and add what is specific to this kernel:
/// `noise` takes the `negative` shape among others, which exercises the `TEST_MIN_NOISE` clamp both
/// paths must apply identically, and pixels are forced onto the exact threshold at both word edges
/// and in the tail, where a strict `>` must leave them unset.
fn assert_mode_matches_scalar(tier: Tier, with_bg: bool) {
    let sigma = 3.0f32;
    for shape in DATA_SHAPES {
        for width in sweep_widths() {
            let mut pixels = shape.row(width, 0);
            let bg = shape.row(width, 1);
            let noise = shape.row(width, 2);
            for index in [0, 1, 63, 64, 65, width / 2, width - 1] {
                if index < width {
                    pixels[index] = threshold_at(with_bg, bg[index], noise[index], sigma);
                }
            }

            // One word past the row, as a `BitBuffer2` row's padding has, primed with ones: the
            // kernel must clear it.
            let words_len = width.div_ceil(64) + 1;
            let mut backend_words = vec![u64::MAX; words_len];
            let mut scalar_words = vec![0u64; words_len];
            let threshold = test_params(sigma);

            if with_bg {
                tier.run(ProcessWords::<true> {
                    pixels: &pixels,
                    bg: &bg,
                    noise: &noise,
                    threshold,
                    words: &mut backend_words,
                });
                process_words_scalar::<true>(&pixels, &bg, &noise, threshold, &mut scalar_words);
            } else {
                tier.run(ProcessWords::<false> {
                    pixels: &pixels,
                    bg: &[],
                    noise: &noise,
                    threshold,
                    words: &mut backend_words,
                });
                process_words_scalar::<false>(&pixels, &[], &noise, threshold, &mut scalar_words);
            }

            assert_eq!(
                backend_words, scalar_words,
                "{tier} vs scalar, shape {} w={width} with_bg={with_bg}",
                shape.name
            );
        }
    }
}

/// Both modes on every tier this CPU has, against the scalar reference.
#[test]
fn every_tier_matches_scalar_packed() {
    for tier in Tier::supported() {
        assert_mode_matches_scalar(tier, true);
        assert_mode_matches_scalar(tier, false);
    }
}
