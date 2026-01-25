//! SIMD-accelerated convolution implementations.
//!
//! This module provides runtime dispatch to the best available SIMD implementation:
//! - AVX2/SSE on x86_64
//! - NEON on aarch64
//! - Scalar fallback on other platforms

#[cfg(target_arch = "x86_64")]
use crate::common::cpu_features;

#[cfg(target_arch = "x86_64")]
pub mod sse;

#[cfg(target_arch = "aarch64")]
pub mod neon;

/// Convolve a single row with 1D kernel using SIMD when available.
///
/// Falls back to scalar implementation on unsupported platforms.
#[inline]
pub fn convolve_row_simd(input: &[f32], output: &mut [f32], kernel: &[f32], radius: usize) {
    #[cfg(target_arch = "x86_64")]
    {
        if cpu_features::has_avx2_fma() {
            unsafe {
                sse::convolve_row_avx2(input, output, kernel, radius);
            }
            return;
        }
        if cpu_features::has_sse4_1() {
            unsafe {
                sse::convolve_row_sse41(input, output, kernel, radius);
            }
            return;
        }
    }

    #[cfg(target_arch = "aarch64")]
    {
        unsafe {
            neon::convolve_row_neon(input, output, kernel, radius);
        }
        return;
    }

    // Scalar fallback
    #[allow(unreachable_code)]
    convolve_row_scalar(input, output, kernel, radius);
}

/// Scalar implementation of row convolution.
#[inline]
fn convolve_row_scalar(input: &[f32], output: &mut [f32], kernel: &[f32], radius: usize) {
    let width = input.len();

    for (x, out) in output.iter_mut().enumerate() {
        let mut sum = 0.0f32;

        for (k, &kval) in kernel.iter().enumerate() {
            let sx = x as isize + k as isize - radius as isize;

            // Mirror boundary handling
            let sx = if sx < 0 {
                (-sx) as usize
            } else if sx >= width as isize {
                2 * width - 2 - sx as usize
            } else {
                sx as usize
            };

            sum += input[sx] * kval;
        }

        *out = sum;
    }
}

/// Check if SIMD is available on this platform.
#[allow(dead_code)]
pub fn simd_available() -> bool {
    #[cfg(target_arch = "x86_64")]
    {
        cpu_features::has_sse4_1()
    }
    #[cfg(target_arch = "aarch64")]
    {
        true // NEON is always available on aarch64
    }
    #[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
    {
        false
    }
}

/// Get the name of the SIMD implementation being used.
#[allow(dead_code)]
pub fn simd_implementation_name() -> &'static str {
    #[cfg(target_arch = "x86_64")]
    {
        if cpu_features::has_avx2_fma() {
            return "AVX2+FMA";
        }
        if cpu_features::has_sse4_1() {
            return "SSE4.1";
        }
    }
    #[cfg(target_arch = "aarch64")]
    {
        return "NEON";
    }
    "Scalar"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_simd_available() {
        // Just verify this doesn't panic
        let _ = simd_available();
        let _ = simd_implementation_name();
    }

    #[test]
    fn test_convolve_row_scalar_identity() {
        let input = vec![1.0, 2.0, 3.0, 4.0, 5.0];
        let kernel = vec![0.0, 1.0, 0.0]; // Identity kernel
        let mut output = vec![0.0; 5];

        convolve_row_scalar(&input, &mut output, &kernel, 1);

        for i in 0..5 {
            assert!(
                (output[i] - input[i]).abs() < 1e-6,
                "Identity kernel should preserve values"
            );
        }
    }

    #[test]
    fn test_convolve_row_scalar_average() {
        let input = vec![0.0, 0.0, 3.0, 0.0, 0.0];
        let kernel = vec![1.0 / 3.0, 1.0 / 3.0, 1.0 / 3.0]; // Average kernel
        let mut output = vec![0.0; 5];

        convolve_row_scalar(&input, &mut output, &kernel, 1);

        // Center pixel should be 1.0 (3.0 / 3.0)
        assert!((output[2] - 1.0).abs() < 1e-6);
        // Neighbors should be 1.0 (3.0 / 3.0)
        assert!((output[1] - 1.0).abs() < 1e-6);
        assert!((output[3] - 1.0).abs() < 1e-6);
    }

    #[test]
    fn test_convolve_row_simd_matches_scalar() {
        let input: Vec<f32> = (0..100).map(|i| i as f32 * 0.1).collect();
        let kernel = vec![0.1, 0.2, 0.4, 0.2, 0.1]; // Gaussian-like
        let radius = 2;

        let mut output_simd = vec![0.0f32; 100];
        let mut output_scalar = vec![0.0f32; 100];

        convolve_row_simd(&input, &mut output_simd, &kernel, radius);
        convolve_row_scalar(&input, &mut output_scalar, &kernel, radius);

        for i in 0..100 {
            assert!(
                (output_simd[i] - output_scalar[i]).abs() < 1e-5,
                "SIMD and scalar should match at index {}: {} vs {}",
                i,
                output_simd[i],
                output_scalar[i]
            );
        }
    }
}
