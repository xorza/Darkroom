//! SSE4.1 and AVX2 SIMD implementations for star detection.

#![allow(clippy::needless_range_loop)]

#[cfg(target_arch = "x86_64")]
use std::arch::x86_64::*;

/// Create threshold mask using AVX2 SIMD.
///
/// Computes: mask[i] = pixels[i] > (background[i] + sigma * noise[i])
///
/// # Safety
/// - Caller must ensure AVX2 is available.
/// - All slices must have the same length.
/// - Length should be >= 8 for efficiency.
#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx2")]
pub unsafe fn create_threshold_mask_avx2(
    pixels: &[f32],
    background: &[f32],
    noise: &[f32],
    sigma: f32,
    output: &mut [bool],
) {
    unsafe {
        let len = pixels.len();
        let sigma_vec = _mm256_set1_ps(sigma);
        let min_noise = _mm256_set1_ps(1e-6);

        let chunks = len / 8;
        for i in 0..chunks {
            let offset = i * 8;

            let px = _mm256_loadu_ps(pixels.as_ptr().add(offset));
            let bg = _mm256_loadu_ps(background.as_ptr().add(offset));
            let n = _mm256_loadu_ps(noise.as_ptr().add(offset));

            // noise = max(noise, 1e-6)
            let n_clamped = _mm256_max_ps(n, min_noise);

            // threshold = bg + sigma * noise
            let threshold = _mm256_add_ps(bg, _mm256_mul_ps(sigma_vec, n_clamped));

            // mask = pixels > threshold
            let cmp = _mm256_cmp_ps(px, threshold, _CMP_GT_OQ);

            // Extract comparison results to booleans
            let mask_bits = _mm256_movemask_ps(cmp) as u8;

            output[offset] = (mask_bits & 0x01) != 0;
            output[offset + 1] = (mask_bits & 0x02) != 0;
            output[offset + 2] = (mask_bits & 0x04) != 0;
            output[offset + 3] = (mask_bits & 0x08) != 0;
            output[offset + 4] = (mask_bits & 0x10) != 0;
            output[offset + 5] = (mask_bits & 0x20) != 0;
            output[offset + 6] = (mask_bits & 0x40) != 0;
            output[offset + 7] = (mask_bits & 0x80) != 0;
        }

        // Handle remainder
        let remainder_start = chunks * 8;
        for i in remainder_start..len {
            let threshold = background[i] + sigma * noise[i].max(1e-6);
            output[i] = pixels[i] > threshold;
        }
    }
}

/// Create threshold mask using SSE4.1 SIMD.
///
/// Computes: mask[i] = pixels[i] > (background[i] + sigma * noise[i])
///
/// # Safety
/// - Caller must ensure SSE4.1 is available.
/// - All slices must have the same length.
/// - Length should be >= 4 for efficiency.
#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "sse4.1")]
pub unsafe fn create_threshold_mask_sse41(
    pixels: &[f32],
    background: &[f32],
    noise: &[f32],
    sigma: f32,
    output: &mut [bool],
) {
    unsafe {
        let len = pixels.len();
        let sigma_vec = _mm_set1_ps(sigma);
        let min_noise = _mm_set1_ps(1e-6);

        let chunks = len / 4;
        for i in 0..chunks {
            let offset = i * 4;

            let px = _mm_loadu_ps(pixels.as_ptr().add(offset));
            let bg = _mm_loadu_ps(background.as_ptr().add(offset));
            let n = _mm_loadu_ps(noise.as_ptr().add(offset));

            // noise = max(noise, 1e-6)
            let n_clamped = _mm_max_ps(n, min_noise);

            // threshold = bg + sigma * noise
            let threshold = _mm_add_ps(bg, _mm_mul_ps(sigma_vec, n_clamped));

            // mask = pixels > threshold
            let cmp = _mm_cmpgt_ps(px, threshold);

            // Extract comparison results to booleans
            let mask_bits = _mm_movemask_ps(cmp) as u8;

            output[offset] = (mask_bits & 0x01) != 0;
            output[offset + 1] = (mask_bits & 0x02) != 0;
            output[offset + 2] = (mask_bits & 0x04) != 0;
            output[offset + 3] = (mask_bits & 0x08) != 0;
        }

        // Handle remainder
        let remainder_start = chunks * 4;
        for i in remainder_start..len {
            let threshold = background[i] + sigma * noise[i].max(1e-6);
            output[i] = pixels[i] > threshold;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_threshold_mask_scalar(
        pixels: &[f32],
        background: &[f32],
        noise: &[f32],
        sigma: f32,
    ) -> Vec<bool> {
        pixels
            .iter()
            .zip(background.iter())
            .zip(noise.iter())
            .map(|((&px, &bg), &n)| {
                let threshold = bg + sigma * n.max(1e-6);
                px > threshold
            })
            .collect()
    }

    #[test]
    #[cfg(target_arch = "x86_64")]
    fn test_avx2_threshold_mask() {
        if !is_x86_feature_detected!("avx2") {
            eprintln!("Skipping AVX2 test - not available");
            return;
        }

        let len = 100;
        let pixels: Vec<f32> = (0..len).map(|i| (i as f32) * 0.01).collect();
        let background: Vec<f32> = (0..len).map(|i| (i as f32) * 0.005).collect();
        let noise: Vec<f32> = (0..len).map(|_| 0.05).collect();
        let sigma = 3.0;

        let expected = create_threshold_mask_scalar(&pixels, &background, &noise, sigma);
        let mut output = vec![false; len];

        unsafe {
            create_threshold_mask_avx2(&pixels, &background, &noise, sigma, &mut output);
        }

        for i in 0..len {
            assert_eq!(
                output[i], expected[i],
                "Mismatch at index {}: got {}, expected {}",
                i, output[i], expected[i]
            );
        }
    }

    #[test]
    #[cfg(target_arch = "x86_64")]
    fn test_sse41_threshold_mask() {
        if !is_x86_feature_detected!("sse4.1") {
            eprintln!("Skipping SSE4.1 test - not available");
            return;
        }

        let len = 50;
        let pixels: Vec<f32> = (0..len).map(|i| (i as f32) * 0.02).collect();
        let background: Vec<f32> = (0..len).map(|i| (i as f32) * 0.01).collect();
        let noise: Vec<f32> = (0..len).map(|_| 0.03).collect();
        let sigma = 2.0;

        let expected = create_threshold_mask_scalar(&pixels, &background, &noise, sigma);
        let mut output = vec![false; len];

        unsafe {
            create_threshold_mask_sse41(&pixels, &background, &noise, sigma, &mut output);
        }

        for i in 0..len {
            assert_eq!(
                output[i], expected[i],
                "Mismatch at index {}: got {}, expected {}",
                i, output[i], expected[i]
            );
        }
    }

    #[test]
    #[cfg(target_arch = "x86_64")]
    fn test_avx2_detects_stars_above_threshold() {
        if !is_x86_feature_detected!("avx2") {
            eprintln!("Skipping AVX2 star detection test - not available");
            return;
        }

        // Background = 0.1, noise = 0.01, sigma = 5.0
        // Threshold = 0.1 + 5.0 * 0.01 = 0.15
        let len = 32;
        let mut pixels = vec![0.1f32; len];
        pixels[10] = 0.2; // Above threshold
        pixels[15] = 0.16; // Just above threshold
        pixels[20] = 0.14; // Below threshold

        let background = vec![0.1f32; len];
        let noise = vec![0.01f32; len];
        let sigma = 5.0;

        let mut output = vec![false; len];
        unsafe {
            create_threshold_mask_avx2(&pixels, &background, &noise, sigma, &mut output);
        }

        assert!(output[10], "Bright pixel should be detected");
        assert!(output[15], "Just-above-threshold pixel should be detected");
        assert!(!output[20], "Below-threshold pixel should not be detected");
        assert!(!output[0], "Background pixel should not be detected");
    }
}
