//! ARM NEON SIMD implementations for star detection.

#[cfg(target_arch = "aarch64")]
use std::arch::aarch64::*;

/// Create threshold mask using NEON SIMD.
///
/// Computes: mask[i] = pixels[i] > (background[i] + sigma * noise[i])
///
/// # Safety
/// - All slices must have the same length.
/// - Length should be >= 4 for efficiency.
#[cfg(target_arch = "aarch64")]
#[target_feature(enable = "neon")]
pub unsafe fn create_threshold_mask_neon(
    pixels: &[f32],
    background: &[f32],
    noise: &[f32],
    sigma: f32,
    output: &mut [bool],
) {
    unsafe {
        let len = pixels.len();
        let sigma_vec = vdupq_n_f32(sigma);
        let min_noise = vdupq_n_f32(1e-6);

        let chunks = len / 4;
        for i in 0..chunks {
            let offset = i * 4;

            let px = vld1q_f32(pixels.as_ptr().add(offset));
            let bg = vld1q_f32(background.as_ptr().add(offset));
            let n = vld1q_f32(noise.as_ptr().add(offset));

            // noise = max(noise, 1e-6)
            let n_clamped = vmaxq_f32(n, min_noise);

            // threshold = bg + sigma * noise
            let threshold = vaddq_f32(bg, vmulq_f32(sigma_vec, n_clamped));

            // mask = pixels > threshold
            let cmp = vcgtq_f32(px, threshold);

            // Extract comparison results
            // NEON comparison returns all-1s or all-0s per lane
            let cmp_u32: uint32x4_t = vreinterpretq_u32_f32(vreinterpretq_f32_u32(cmp));

            // Extract each lane
            output[offset] = vgetq_lane_u32(cmp_u32, 0) != 0;
            output[offset + 1] = vgetq_lane_u32(cmp_u32, 1) != 0;
            output[offset + 2] = vgetq_lane_u32(cmp_u32, 2) != 0;
            output[offset + 3] = vgetq_lane_u32(cmp_u32, 3) != 0;
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
    #[allow(unused_imports)]
    use super::*;

    #[cfg(target_arch = "aarch64")]
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
    #[cfg(target_arch = "aarch64")]
    fn test_neon_threshold_mask() {
        let len = 50;
        let pixels: Vec<f32> = (0..len).map(|i| (i as f32) * 0.02).collect();
        let background: Vec<f32> = (0..len).map(|i| (i as f32) * 0.01).collect();
        let noise: Vec<f32> = (0..len).map(|_| 0.03).collect();
        let sigma = 2.0;

        let expected = create_threshold_mask_scalar(&pixels, &background, &noise, sigma);
        let mut output = vec![false; len];

        unsafe {
            create_threshold_mask_neon(&pixels, &background, &noise, sigma, &mut output);
        }

        for i in 0..len {
            assert_eq!(
                output[i], expected[i],
                "Mismatch at index {}: got {}, expected {}",
                i, output[i], expected[i]
            );
        }
    }
}
