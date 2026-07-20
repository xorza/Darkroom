use crate::stacking::registration::resample::kernel;
use crate::stacking::registration::resample::row;
use crate::stacking::registration::resample::row::sse;
use crate::stacking::registration::transform::{Transform, WarpTransform};
use crate::testing::synthetic::patterns;
use glam::DVec2;
use imaginarium::Buffer2;
#[cfg(target_arch = "x86_64")]
use imaginarium::cpu_features;

/// Helper: compare SIMD output against scalar reference for a given transform.
#[cfg(target_arch = "x86_64")]
fn assert_avx2_matches_scalar(
    input: &Buffer2<f32>,
    transform: &Transform,
    y: usize,
    tol: f32,
    label: &str,
) {
    if !cpu_features::has_avx2() {
        return;
    }
    let width = input.width();
    let inverse = transform.inverse();
    let mut output_avx2 = vec![0.0f32; width];
    let mut output_scalar = vec![0.0f32; width];

    unsafe {
        sse::bilinear_avx2(input, &mut output_avx2, y, &inverse);
    }
    let inverse_wt = WarpTransform::new(inverse);
    row::bilinear_scalar(input, &mut output_scalar, y, &inverse_wt, 0.0);

    for x in 0..width {
        assert!(
            (output_avx2[x] - output_scalar[x]).abs() < tol,
            "{label}: x={x}: AVX2 {} vs Scalar {}",
            output_avx2[x],
            output_scalar[x]
        );
    }
}

#[cfg(target_arch = "x86_64")]
fn assert_sse_matches_scalar(
    input: &Buffer2<f32>,
    transform: &Transform,
    y: usize,
    tol: f32,
    label: &str,
) {
    if !cpu_features::has_sse4_1() {
        return;
    }
    let width = input.width();
    let inverse = transform.inverse();
    let mut output_sse = vec![0.0f32; width];
    let mut output_scalar = vec![0.0f32; width];

    unsafe {
        sse::bilinear_sse(input, &mut output_sse, y, &inverse);
    }
    let inverse_wt = WarpTransform::new(inverse);
    row::bilinear_scalar(input, &mut output_scalar, y, &inverse_wt, 0.0);

    for x in 0..width {
        assert!(
            (output_sse[x] - output_scalar[x]).abs() < tol,
            "{label}: x={x}: SSE {} vs Scalar {}",
            output_sse[x],
            output_scalar[x]
        );
    }
}

#[test]
#[cfg(target_arch = "x86_64")]
fn test_avx2_warp_row_translation() {
    let input = patterns::diagonal_gradient(128, 64);
    let transform = Transform::translation(DVec2::new(2.5, 1.5));
    assert_avx2_matches_scalar(&input, &transform, 30, 1e-5, "AVX2 translation");
}

#[test]
#[cfg(target_arch = "x86_64")]
fn test_avx2_warp_row_identity() {
    let input = patterns::diagonal_gradient(128, 64);
    let transform = Transform::identity();
    assert_avx2_matches_scalar(&input, &transform, 30, 1e-5, "AVX2 identity");
}

#[test]
#[cfg(target_arch = "x86_64")]
fn test_avx2_warp_row_similarity() {
    let input = patterns::diagonal_gradient(128, 64);
    let transform = Transform::similarity(DVec2::new(3.0, 2.0), 0.1, 1.05);
    assert_avx2_matches_scalar(&input, &transform, 30, 1e-4, "AVX2 similarity");
}

#[test]
#[cfg(target_arch = "x86_64")]
fn test_avx2_warp_row_remainder_pixels() {
    // Width not a multiple of 8: tests the scalar remainder path.
    // Width=13: 1 chunk of 8 + 5 remainder pixels.
    let input = patterns::diagonal_gradient(13, 32);
    let transform = Transform::translation(DVec2::new(1.5, 0.5));
    assert_avx2_matches_scalar(&input, &transform, 15, 1e-5, "AVX2 width=13");
}

#[test]
#[cfg(target_arch = "x86_64")]
fn test_sse_warp_row_similarity() {
    let input = patterns::diagonal_gradient(64, 64);
    let transform = Transform::similarity(DVec2::new(1.0, 2.0), 0.05, 1.02);
    assert_sse_matches_scalar(&input, &transform, 25, 1e-5, "SSE similarity");
}

#[test]
#[cfg(target_arch = "x86_64")]
fn test_sse_warp_row_identity() {
    let input = patterns::diagonal_gradient(64, 64);
    let transform = Transform::identity();
    assert_sse_matches_scalar(&input, &transform, 25, 1e-5, "SSE identity");
}

#[test]
#[cfg(target_arch = "x86_64")]
fn test_sse_warp_row_remainder_pixels() {
    // Width not a multiple of 4: tests the scalar remainder path.
    // Width=11: 2 chunks of 4 + 3 remainder pixels.
    let input = patterns::diagonal_gradient(11, 32);
    let transform = Transform::translation(DVec2::new(1.5, 0.5));
    assert_sse_matches_scalar(&input, &transform, 15, 1e-5, "SSE width=11");
}

/// Helper: compute scalar Lanczos weighted sum and compare against SIMD kernel.
#[cfg(target_arch = "x86_64")]
fn assert_lanczos_kernel_fma_matches_scalar<const A: usize, const SIZE: usize>(label: &str) {
    if !cpu_features::has_avx2_fma() {
        return;
    }

    // 20x20 image: pixel(x, y) = x + y * 0.1
    let width = 20;
    let height = 20;
    let data: Vec<f32> = (0..width * height)
        .map(|i| {
            let x = (i % width) as f32;
            let y = (i / width) as f32;
            x + y * 0.1
        })
        .collect();

    let lut = kernel::get_lanczos_lut(A);
    let a_minus_1 = A as i32 - 1;

    // Test at interior position: x0=6, y0=6, fx=0.3, fy=0.7
    // kx0 = x0 - (A-1), ky0 = y0 - (A-1)
    let kx = (6 - a_minus_1) as usize;
    let ky = (6 - a_minus_1) as usize;
    let fx = 0.3f32;
    let fy = 0.7f32;

    // Compute weights same as lanczos_inner
    let mut wx = [0.0f32; SIZE];
    let mut wy = [0.0f32; SIZE];
    for i in 0..SIZE {
        wx[i] = if i < A {
            lut.lookup_positive((a_minus_1 - i as i32) as f32 + fx)
        } else {
            lut.lookup_positive((i as i32 - a_minus_1) as f32 - fx)
        };
        wy[i] = if i < A {
            lut.lookup_positive((a_minus_1 - i as i32) as f32 + fy)
        } else {
            lut.lookup_positive((i as i32 - a_minus_1) as f32 - fy)
        };
    }

    let mut scalar_sum = 0.0f32;
    for j in 0..SIZE {
        for k in 0..SIZE {
            let v = data[(ky + j) * width + kx + k];
            scalar_sum += v * wx[k] * wy[j];
        }
    }

    let simd_acc = unsafe { sse::lanczos_kernel_fma::<SIZE>(&data, width, kx, ky, &wx, &wy) };
    assert!(
        (simd_acc - scalar_sum).abs() < 1e-4,
        "{label}: SIMD {simd_acc} vs scalar {scalar_sum}",
    );
}

#[test]
#[cfg(target_arch = "x86_64")]
fn test_lanczos2_kernel_fma_matches_scalar() {
    assert_lanczos_kernel_fma_matches_scalar::<2, 4>("Lanczos2");
}

#[test]
#[cfg(target_arch = "x86_64")]
fn test_lanczos3_kernel_fma_matches_scalar() {
    assert_lanczos_kernel_fma_matches_scalar::<3, 6>("Lanczos3");
}

#[test]
#[cfg(target_arch = "x86_64")]
fn test_lanczos4_kernel_fma_matches_scalar() {
    assert_lanczos_kernel_fma_matches_scalar::<4, 8>("Lanczos4");
}
