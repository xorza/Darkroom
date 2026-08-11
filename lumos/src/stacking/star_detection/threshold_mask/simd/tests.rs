//! Cross-checks that every backend's packed words match the scalar reference exactly.

use crate::stacking::star_detection::threshold_mask::simd::{MIN_NOISE, process_words_scalar};
use crate::testing::simd_check::{DATA_SHAPES, SWEEP_WIDTHS};

/// The shared sweep plus widths spanning whole 64-pixel words. Every backend vectorizes full words
/// and hands whatever is left to `process_words_scalar`, so a word exactly filled, a word and a
/// little, and several words with an odd tail are the boundaries that matter here — the shared list
/// alone never reaches two full words.
fn sweep_widths() -> Vec<usize> {
    SWEEP_WIDTHS
        .iter()
        .copied()
        .chain([65, 127, 128, 130, 193])
        .collect()
}

/// The threshold the kernels compare against, written as they write it so a pixel set to it lands
/// exactly on the boundary rather than near it.
fn threshold_at(with_bg: bool, bg: f32, noise: f32, sigma: f32) -> f32 {
    let threshold = sigma * noise.max(MIN_NOISE);
    if with_bg { threshold + bg } else { threshold }
}

/// One backend mode against the scalar reference, over every shared data shape and width.
///
/// Compared exactly rather than within a tolerance: a packed word is a set of detections, so one
/// differing bit is one pixel the two paths disagree about. That is also why the backends compute
/// `bg + σ·noise` unfused — a contracted multiply-add would round differently from this reference.
///
/// The inputs lean on the shared shapes for coverage, and add what is specific to this kernel:
/// `noise` takes the `negative` shape among others, which exercises the [`MIN_NOISE`] clamp both
/// paths must apply identically, and pixels are forced onto the exact threshold at both word edges
/// and in the tail, where a strict `>` must leave them unset.
fn assert_mode_matches_scalar(
    name: &str,
    with_bg: bool,
    run_backend: impl Fn(&[f32], &[f32], &[f32], f32, &mut [u64], usize),
) {
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

            let words_len = width.div_ceil(64);
            let mut backend_words = vec![0u64; words_len];
            let mut scalar_words = vec![0u64; words_len];

            run_backend(&pixels, &bg, &noise, sigma, &mut backend_words, width);
            if with_bg {
                process_words_scalar::<true>(
                    &pixels,
                    &bg,
                    &noise,
                    sigma,
                    &mut scalar_words,
                    0,
                    width,
                );
            } else {
                process_words_scalar::<false>(
                    &pixels,
                    &[],
                    &noise,
                    sigma,
                    &mut scalar_words,
                    0,
                    width,
                );
            }

            assert_eq!(
                backend_words, scalar_words,
                "{name} vs scalar, shape {} w={width} with_bg={with_bg}",
                shape.name
            );
        }
    }
}

/// Run a backend's two threshold modes against the scalar reference. Takes the kernel by name, as
/// brought into scope by the caller's `use`.
///
/// A macro because the mode is a const generic: writing the `::<true>`/`::<false>` pairing here once
/// is what keeps a caller from handing the harness one mode's kernel while claiming the other, and
/// from passing a `bg` slice to the kernel that ignores it.
macro_rules! assert_backend_matches_scalar {
    ($name:literal, $backend:ident) => {
        assert_mode_matches_scalar($name, true, |pixels, bg, noise, sigma, words, end| unsafe {
            $backend::<true>(pixels, bg, noise, sigma, words, 0, end)
        });
        assert_mode_matches_scalar(
            $name,
            false,
            |pixels, _bg, noise, sigma, words, end| unsafe {
                $backend::<false>(pixels, &[], noise, sigma, words, 0, end)
            },
        );
    };
}

#[cfg(target_arch = "x86_64")]
#[test]
fn avx2_matches_scalar_packed() {
    if !imaginarium::cpu_features::has_avx2() {
        return; // backend not present on this host
    }
    use crate::stacking::star_detection::threshold_mask::simd::avx2::process_words_avx2;
    assert_backend_matches_scalar!("AVX2", process_words_avx2);
}

#[cfg(target_arch = "x86_64")]
#[test]
fn sse41_matches_scalar_packed() {
    if !imaginarium::cpu_features::has_sse4_1() {
        return; // backend not present on this host
    }
    use crate::stacking::star_detection::threshold_mask::simd::sse41::process_words_sse;
    assert_backend_matches_scalar!("SSE4.1", process_words_sse);
}

#[cfg(target_arch = "aarch64")]
#[test]
fn neon_matches_scalar_packed() {
    use crate::stacking::star_detection::threshold_mask::simd::neon::process_words_neon;
    assert_backend_matches_scalar!("NEON", process_words_neon);
}
