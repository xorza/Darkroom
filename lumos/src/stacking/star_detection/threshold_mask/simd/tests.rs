//! Cross-checks that every backend's packed words match the scalar reference exactly.

#[cfg(target_arch = "x86_64")]
use crate::stacking::star_detection::threshold_mask::simd::avx2::process_words_avx2;
#[cfg(target_arch = "x86_64")]
use crate::stacking::star_detection::threshold_mask::simd::process_words_scalar;

/// The AVX2 packed kernel must produce bit-identical words to the scalar reference, including the
/// partial-word tail and `px == threshold` boundary pixels (strict `>`, so never set).
#[cfg(target_arch = "x86_64")]
#[test]
fn avx2_matches_scalar_packed() {
    if !imaginarium::cpu_features::has_avx2() {
        return; // backend not present on this host
    }

    // 130 px = 2 full 64-px words + a 2-bit partial word → exercises the scalar tail path.
    let n = 130;
    let sigma = 3.0f32;
    let mut pixels = vec![0.0f32; n];
    let mut bg = vec![0.0f32; n];
    let mut noise = vec![0.0f32; n];
    for i in 0..n {
        let h = (i as u32).wrapping_mul(2654435761);
        pixels[i] = (h as f32 / u32::MAX as f32) * 2.0; // [0, 2)
        bg[i] = 0.1 + (i % 5) as f32 * 0.05;
        noise[i] = 0.02 + (i % 7) as f32 * 0.01;
    }
    // Force exact-boundary pixels (px == bg + σ·noise) across a word edge and the tail.
    for &i in &[3usize, 63, 64, 70, 129] {
        pixels[i] = bg[i] + sigma * noise[i].max(1e-6);
    }

    let words_len = n.div_ceil(64);
    let mut w_avx = vec![0u64; words_len];
    let mut w_scalar = vec![0u64; words_len];

    unsafe { process_words_avx2::<true>(&pixels, &bg, &noise, sigma, &mut w_avx, 0, n) };
    process_words_scalar::<true>(&pixels, &bg, &noise, sigma, &mut w_scalar, 0, n);
    assert_eq!(w_avx, w_scalar, "AVX2 vs scalar mismatch (WITH_BG=true)");

    // Matched-filter case: WITH_BG=false, bg empty, threshold = σ·noise.
    w_avx.iter_mut().for_each(|w| *w = 0);
    w_scalar.iter_mut().for_each(|w| *w = 0);
    unsafe { process_words_avx2::<false>(&pixels, &[], &noise, sigma, &mut w_avx, 0, n) };
    process_words_scalar::<false>(&pixels, &[], &noise, sigma, &mut w_scalar, 0, n);
    assert_eq!(w_avx, w_scalar, "AVX2 vs scalar mismatch (WITH_BG=false)");
}
