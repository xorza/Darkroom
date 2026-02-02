//! SIMD-optimized centroid refinement implementations.
//!
//! This module provides platform-specific SIMD implementations for
//! the centroid refinement inner loop, with automatic runtime dispatch
//! to the best available implementation.

mod scalar;

#[cfg(target_arch = "x86_64")]
mod avx2;
#[cfg(target_arch = "x86_64")]
mod sse;

#[cfg(target_arch = "aarch64")]
mod neon;

#[cfg(test)]
mod bench;

use common::cpu_features;

use crate::star_detection::background::BackgroundMap;

pub use scalar::refine_centroid_scalar;

#[cfg(target_arch = "x86_64")]
pub use avx2::refine_centroid_avx2;
#[cfg(target_arch = "x86_64")]
pub use sse::refine_centroid_sse;

#[cfg(target_arch = "aarch64")]
pub use neon::refine_centroid_neon;

/// Refine centroid position using the best available SIMD implementation.
///
/// Automatically dispatches to AVX2, SSE, NEON, or scalar based on
/// runtime CPU feature detection.
#[allow(clippy::too_many_arguments)]
pub fn refine_centroid(
    pixels: &[f32],
    width: usize,
    height: usize,
    background: &BackgroundMap,
    cx: f32,
    cy: f32,
    stamp_radius: usize,
    expected_fwhm: f32,
) -> Option<(f32, f32)> {
    #[cfg(target_arch = "x86_64")]
    {
        if cpu_features::has_avx2_fma() {
            // SAFETY: AVX2 and FMA are available
            return unsafe {
                refine_centroid_avx2(
                    pixels,
                    width,
                    height,
                    background,
                    cx,
                    cy,
                    stamp_radius,
                    expected_fwhm,
                )
            };
        }
        if cpu_features::has_sse4_1() {
            // SAFETY: SSE4.1 is available
            return unsafe {
                refine_centroid_sse(
                    pixels,
                    width,
                    height,
                    background,
                    cx,
                    cy,
                    stamp_radius,
                    expected_fwhm,
                )
            };
        }
    }

    #[cfg(target_arch = "aarch64")]
    {
        // NEON is always available on aarch64
        return unsafe {
            refine_centroid_neon(
                pixels,
                width,
                height,
                background,
                cx,
                cy,
                stamp_radius,
                expected_fwhm,
            )
        };
    }

    refine_centroid_scalar(
        pixels,
        width,
        height,
        background,
        cx,
        cy,
        stamp_radius,
        expected_fwhm,
    )
}
