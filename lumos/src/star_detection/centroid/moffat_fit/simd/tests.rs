//! Tests for SIMD Moffat computations.

use super::{compute_chi2_scalar, fill_jacobian_residuals_scalar};

fn approx_eq(a: f32, b: f32, rel_tolerance: f32, abs_tolerance: f32) -> bool {
    let diff = (a - b).abs();
    diff < abs_tolerance || diff / a.abs().max(b.abs()).max(1e-10) < rel_tolerance
}

/// Compute scalar Jacobian row for comparison
fn scalar_jacobian(x: f32, y: f32, params: &[f32; 5], beta: f32) -> [f32; 5] {
    let [x0, y0, amp, alpha, _bg] = *params;
    let alpha2 = alpha * alpha;
    let dx = x - x0;
    let dy = y - y0;
    let r2 = dx * dx + dy * dy;
    let u = 1.0 + r2 / alpha2;
    let u_neg_beta = u.powf(-beta);
    let u_neg_beta_m1 = u_neg_beta / u;
    let common = 2.0 * amp * beta / alpha2 * u_neg_beta_m1;
    [
        common * dx,
        common * dy,
        u_neg_beta,
        common * r2 / alpha,
        1.0,
    ]
}

/// Compute scalar model value
fn scalar_model(x: f32, y: f32, params: &[f32; 5], beta: f32) -> f32 {
    let [x0, y0, amp, alpha, bg] = *params;
    let r2 = (x - x0).powi(2) + (y - y0).powi(2);
    amp * (1.0 + r2 / (alpha * alpha)).powf(-beta) + bg
}

/// Generate test data for n pixels
fn generate_test_data(n: usize, params: &[f32; 5], beta: f32) -> (Vec<f32>, Vec<f32>, Vec<f32>) {
    let side = ((n as f32).sqrt().ceil() as usize).max(1);
    let offset = params[0] - (side as f32 / 2.0);

    let mut data_x = Vec::with_capacity(n);
    let mut data_y = Vec::with_capacity(n);
    let mut data_z = Vec::with_capacity(n);

    for i in 0..n {
        let x = (i % side) as f32 + offset;
        let y = (i / side) as f32 + offset;
        data_x.push(x);
        data_y.push(y);
        data_z.push(scalar_model(x, y, params, beta));
    }

    (data_x, data_y, data_z)
}

// ============================================================================
// Scalar fallback tests
// ============================================================================

mod scalar_tests {
    use super::*;

    #[test]
    fn test_scalar_jacobian_matches_reference() {
        let params = [8.0f32, 8.0, 1.0, 2.5, 0.1];
        let beta = 2.5f32;
        let (data_x, data_y, data_z) = generate_test_data(16, &params, beta);

        let mut jacobian = Vec::new();
        let mut residuals = Vec::new();
        fill_jacobian_residuals_scalar(
            &data_x,
            &data_y,
            &data_z,
            &params,
            beta,
            &mut jacobian,
            &mut residuals,
        );

        for i in 0..16 {
            let j_ref = scalar_jacobian(data_x[i], data_y[i], &params, beta);
            for j in 0..5 {
                assert!(
                    approx_eq(jacobian[i][j], j_ref[j], 1e-5, 1e-6),
                    "Jacobian[{}][{}] mismatch: got={}, expected={}",
                    i,
                    j,
                    jacobian[i][j],
                    j_ref[j]
                );
            }
        }
    }

    #[test]
    fn test_scalar_chi2_matches_reference() {
        let params = [8.0f32, 8.0, 1.0, 2.5, 0.1];
        let beta = 2.5f32;
        let (data_x, data_y, mut data_z) = generate_test_data(16, &params, beta);

        // Add noise to get non-zero chi2
        for (i, z) in data_z.iter_mut().enumerate() {
            *z += 0.01 * (i as f32);
        }

        let chi2 = compute_chi2_scalar(&data_x, &data_y, &data_z, &params, beta);

        // Compute reference
        let mut chi2_ref = 0.0f32;
        for i in 0..data_x.len() {
            let model = scalar_model(data_x[i], data_y[i], &params, beta);
            let residual = data_z[i] - model;
            chi2_ref += residual * residual;
        }

        assert!(
            approx_eq(chi2, chi2_ref, 1e-5, 1e-6),
            "Chi2 mismatch: got={}, expected={}",
            chi2,
            chi2_ref
        );
    }

    #[test]
    fn test_scalar_empty_arrays() {
        let data_x: Vec<f32> = vec![];
        let data_y: Vec<f32> = vec![];
        let data_z: Vec<f32> = vec![];
        let params = [5.0f32, 5.0, 1.0, 2.0, 0.1];
        let beta = 2.5f32;

        let mut jacobian = Vec::new();
        let mut residuals = Vec::new();
        fill_jacobian_residuals_scalar(
            &data_x,
            &data_y,
            &data_z,
            &params,
            beta,
            &mut jacobian,
            &mut residuals,
        );

        assert_eq!(jacobian.len(), 0);
        assert_eq!(residuals.len(), 0);

        let chi2 = compute_chi2_scalar(&data_x, &data_y, &data_z, &params, beta);
        assert_eq!(chi2, 0.0);
    }

    #[test]
    fn test_scalar_single_pixel() {
        let params = [5.0f32, 5.0, 1.0, 2.0, 0.1];
        let beta = 2.5f32;
        let (data_x, data_y, data_z) = generate_test_data(1, &params, beta);

        let mut jacobian = Vec::new();
        let mut residuals = Vec::new();
        fill_jacobian_residuals_scalar(
            &data_x,
            &data_y,
            &data_z,
            &params,
            beta,
            &mut jacobian,
            &mut residuals,
        );

        assert_eq!(jacobian.len(), 1);
        assert_eq!(residuals.len(), 1);
        assert!(
            residuals[0].abs() < 1e-6,
            "Perfect fit should have ~0 residual"
        );
    }
}

// ============================================================================
// Dispatch function tests
// ============================================================================

mod dispatch_tests {
    use super::super::{compute_chi2_simd_fixed_beta, fill_jacobian_residuals_simd_fixed_beta};
    use super::*;

    #[test]
    fn test_dispatch_jacobian_matches_scalar() {
        let params = [8.0f32, 8.0, 1.0, 2.5, 0.1];
        let beta = 2.5f32;
        let (data_x, data_y, data_z) = generate_test_data(17, &params, beta);

        // Get scalar reference
        let mut jacobian_scalar = Vec::new();
        let mut residuals_scalar = Vec::new();
        fill_jacobian_residuals_scalar(
            &data_x,
            &data_y,
            &data_z,
            &params,
            beta,
            &mut jacobian_scalar,
            &mut residuals_scalar,
        );

        // Get dispatch result
        let mut jacobian_simd = Vec::new();
        let mut residuals_simd = Vec::new();
        fill_jacobian_residuals_simd_fixed_beta(
            &data_x,
            &data_y,
            &data_z,
            &params,
            beta,
            &mut jacobian_simd,
            &mut residuals_simd,
        );

        assert_eq!(jacobian_simd.len(), jacobian_scalar.len());
        assert_eq!(residuals_simd.len(), residuals_scalar.len());

        for i in 0..jacobian_scalar.len() {
            for j in 0..5 {
                assert!(
                    approx_eq(jacobian_simd[i][j], jacobian_scalar[i][j], 1e-5, 1e-6),
                    "Dispatch Jacobian[{}][{}] mismatch: simd={}, scalar={}",
                    i,
                    j,
                    jacobian_simd[i][j],
                    jacobian_scalar[i][j]
                );
            }
            assert!(
                approx_eq(residuals_simd[i], residuals_scalar[i], 1e-5, 1e-6),
                "Dispatch residual[{}] mismatch",
                i
            );
        }
    }

    #[test]
    fn test_dispatch_chi2_matches_scalar() {
        let params = [8.0f32, 8.0, 1.0, 2.5, 0.1];
        let beta = 2.5f32;
        let (data_x, data_y, mut data_z) = generate_test_data(17, &params, beta);

        for (i, z) in data_z.iter_mut().enumerate() {
            *z += 0.01 * (i as f32);
        }

        let chi2_scalar = compute_chi2_scalar(&data_x, &data_y, &data_z, &params, beta);
        let chi2_simd = compute_chi2_simd_fixed_beta(&data_x, &data_y, &data_z, &params, beta);

        assert!(
            approx_eq(chi2_simd, chi2_scalar, 1e-5, 1e-6),
            "Dispatch chi2 mismatch: simd={}, scalar={}",
            chi2_simd,
            chi2_scalar
        );
    }

    #[test]
    fn test_dispatch_various_sizes() {
        let params = [8.0f32, 8.0, 1.0, 2.5, 0.1];
        let beta = 2.5f32;

        // Test sizes that exercise different code paths
        for n in [
            0, 1, 3, 4, 5, 7, 8, 9, 15, 16, 17, 24, 25, 31, 32, 33, 100, 289,
        ] {
            let (data_x, data_y, data_z) = generate_test_data(n, &params, beta);

            let chi2_scalar = compute_chi2_scalar(&data_x, &data_y, &data_z, &params, beta);
            let chi2_simd = compute_chi2_simd_fixed_beta(&data_x, &data_y, &data_z, &params, beta);

            assert!(
                approx_eq(chi2_simd, chi2_scalar, 1e-4, 1e-5),
                "n={}: chi2 mismatch: simd={}, scalar={}",
                n,
                chi2_simd,
                chi2_scalar
            );
        }
    }
}

// ============================================================================
// AVX2-specific tests
// ============================================================================

#[cfg(target_arch = "x86_64")]
mod avx2_tests {
    use super::super::avx2::*;
    use super::*;
    use common::cpu_features;

    #[test]
    fn test_jacobian_residuals_matches_scalar() {
        if !cpu_features::has_avx2_fma() {
            println!("AVX2 not available, skipping test");
            return;
        }

        let params = [8.0f32, 8.0, 1.0, 2.5, 0.1];
        let beta = 2.5f32;
        let (data_x, data_y, data_z) = generate_test_data(16, &params, beta);

        let mut jacobian_simd = Vec::new();
        let mut residuals_simd = Vec::new();
        unsafe {
            fill_jacobian_residuals_simd_fixed_beta(
                &data_x,
                &data_y,
                &data_z,
                &params,
                beta,
                &mut jacobian_simd,
                &mut residuals_simd,
            );
        }

        for i in 0..16 {
            let j_scalar = scalar_jacobian(data_x[i], data_y[i], &params, beta);
            let model = scalar_model(data_x[i], data_y[i], &params, beta);
            let residual_scalar = data_z[i] - model;

            assert!(
                approx_eq(residuals_simd[i], residual_scalar, 1e-5, 1e-6),
                "Residual mismatch at {}: simd={}, scalar={}",
                i,
                residuals_simd[i],
                residual_scalar,
            );

            for j in 0..5 {
                assert!(
                    approx_eq(jacobian_simd[i][j], j_scalar[j], 1e-5, 1e-6),
                    "Jacobian[{}][{}] mismatch: simd={}, scalar={}",
                    i,
                    j,
                    jacobian_simd[i][j],
                    j_scalar[j]
                );
            }
        }
    }

    #[test]
    fn test_chi2_matches_scalar() {
        if !cpu_features::has_avx2_fma() {
            println!("AVX2 not available, skipping test");
            return;
        }

        let params = [8.5f32, 8.3, 0.9, 2.2, 0.15];
        let beta = 3.0f32;
        let (data_x, data_y, mut data_z) = generate_test_data(17, &params, beta);

        for (i, z) in data_z.iter_mut().enumerate() {
            *z += 0.01 * (i as f32);
        }

        let chi2_simd =
            unsafe { compute_chi2_simd_fixed_beta(&data_x, &data_y, &data_z, &params, beta) };

        let mut chi2_scalar = 0.0f32;
        for i in 0..data_x.len() {
            let model = scalar_model(data_x[i], data_y[i], &params, beta);
            let residual = data_z[i] - model;
            chi2_scalar += residual * residual;
        }

        assert!(
            approx_eq(chi2_simd, chi2_scalar, 1e-5, 1e-6),
            "Chi2 mismatch: simd={}, scalar={}",
            chi2_simd,
            chi2_scalar
        );
    }

    #[test]
    fn test_handles_small_arrays() {
        if !cpu_features::has_avx2_fma() {
            println!("AVX2 not available, skipping test");
            return;
        }

        // Test with fewer than 8 pixels (pure scalar fallback)
        let params = [7.0f32, 7.0, 0.7, 2.0, 0.1];
        let beta = 2.5f32;
        let (data_x, data_y, data_z) = generate_test_data(5, &params, beta);

        let mut jacobian = Vec::new();
        let mut residuals = Vec::new();
        unsafe {
            fill_jacobian_residuals_simd_fixed_beta(
                &data_x,
                &data_y,
                &data_z,
                &params,
                beta,
                &mut jacobian,
                &mut residuals,
            );
        }

        assert_eq!(jacobian.len(), 5);
        assert_eq!(residuals.len(), 5);

        let chi2 =
            unsafe { compute_chi2_simd_fixed_beta(&data_x, &data_y, &data_z, &params, beta) };
        assert!(chi2.is_finite());
    }

    #[test]
    fn test_exactly_8_pixels() {
        if !cpu_features::has_avx2_fma() {
            println!("AVX2 not available, skipping test");
            return;
        }

        let params = [4.0f32, 4.0, 1.0, 2.0, 0.1];
        let beta = 2.5f32;
        let (data_x, data_y, data_z) = generate_test_data(8, &params, beta);

        let mut jacobian = Vec::new();
        let mut residuals = Vec::new();
        unsafe {
            fill_jacobian_residuals_simd_fixed_beta(
                &data_x,
                &data_y,
                &data_z,
                &params,
                beta,
                &mut jacobian,
                &mut residuals,
            );
        }

        assert_eq!(jacobian.len(), 8);
        for i in 0..8 {
            let j_scalar = scalar_jacobian(data_x[i], data_y[i], &params, beta);
            for j in 0..5 {
                assert!(
                    approx_eq(jacobian[i][j], j_scalar[j], 1e-5, 1e-6),
                    "Jacobian[{}][{}] mismatch",
                    i,
                    j
                );
            }
        }
    }

    #[test]
    fn test_9_pixels_simd_plus_scalar() {
        if !cpu_features::has_avx2_fma() {
            println!("AVX2 not available, skipping test");
            return;
        }

        let params = [5.0f32, 5.0, 0.8, 2.5, 0.1];
        let beta = 2.5f32;
        let (data_x, data_y, data_z) = generate_test_data(9, &params, beta);

        let mut jacobian = Vec::new();
        let mut residuals = Vec::new();
        unsafe {
            fill_jacobian_residuals_simd_fixed_beta(
                &data_x,
                &data_y,
                &data_z,
                &params,
                beta,
                &mut jacobian,
                &mut residuals,
            );
        }

        assert_eq!(jacobian.len(), 9);

        // Verify the 9th element (scalar fallback)
        let j_scalar = scalar_jacobian(data_x[8], data_y[8], &params, beta);
        for j in 0..5 {
            assert!(
                approx_eq(jacobian[8][j], j_scalar[j], 1e-5, 1e-6),
                "Scalar fallback Jacobian[8][{}] mismatch",
                j
            );
        }
    }

    #[test]
    fn test_empty_arrays() {
        if !cpu_features::has_avx2_fma() {
            println!("AVX2 not available, skipping test");
            return;
        }

        let data_x: Vec<f32> = vec![];
        let data_y: Vec<f32> = vec![];
        let data_z: Vec<f32> = vec![];
        let params = [5.0f32, 5.0, 1.0, 2.0, 0.1];
        let beta = 2.5f32;

        let mut jacobian = Vec::new();
        let mut residuals = Vec::new();
        unsafe {
            fill_jacobian_residuals_simd_fixed_beta(
                &data_x,
                &data_y,
                &data_z,
                &params,
                beta,
                &mut jacobian,
                &mut residuals,
            );
        }

        assert_eq!(jacobian.len(), 0);
        assert_eq!(residuals.len(), 0);

        let chi2 =
            unsafe { compute_chi2_simd_fixed_beta(&data_x, &data_y, &data_z, &params, beta) };
        assert_eq!(chi2, 0.0);
    }

    #[test]
    fn test_various_beta_values() {
        if !cpu_features::has_avx2_fma() {
            println!("AVX2 not available, skipping test");
            return;
        }

        let params = [8.0f32, 8.0, 1.0, 2.5, 0.1];

        for beta in [1.5f32, 2.0, 2.5, 3.0, 4.0, 5.0] {
            let (data_x, data_y, data_z) = generate_test_data(16, &params, beta);

            let mut jacobian = Vec::new();
            let mut residuals = Vec::new();
            unsafe {
                fill_jacobian_residuals_simd_fixed_beta(
                    &data_x,
                    &data_y,
                    &data_z,
                    &params,
                    beta,
                    &mut jacobian,
                    &mut residuals,
                );
            }

            for (i, &r) in residuals.iter().enumerate() {
                assert!(
                    r.abs() < 1e-5,
                    "beta={}: residual[{}]={} should be near zero",
                    beta,
                    i,
                    r
                );
            }
        }
    }

    #[test]
    fn test_various_alpha_values() {
        if !cpu_features::has_avx2_fma() {
            println!("AVX2 not available, skipping test");
            return;
        }

        let beta = 2.5f32;

        for alpha in [0.5f32, 1.0, 2.0, 3.0, 5.0, 10.0] {
            let params = [8.0f32, 8.0, 1.0, alpha, 0.1];
            let (data_x, data_y, data_z) = generate_test_data(16, &params, beta);

            let chi2_simd =
                unsafe { compute_chi2_simd_fixed_beta(&data_x, &data_y, &data_z, &params, beta) };

            assert!(
                chi2_simd < 1e-10,
                "alpha={}: chi2={} should be near zero",
                alpha,
                chi2_simd
            );
        }
    }

    #[test]
    fn test_extreme_amplitude_values() {
        if !cpu_features::has_avx2_fma() {
            println!("AVX2 not available, skipping test");
            return;
        }

        let beta = 2.5f32;
        let n = 16;

        for amp in [0.001f32, 0.1, 1.0, 10.0, 1000.0, 65535.0] {
            let params = [8.0f32, 8.0, amp, 2.5, 0.1];
            let (data_x, data_y, data_z) = generate_test_data(n, &params, beta);

            let chi2_simd =
                unsafe { compute_chi2_simd_fixed_beta(&data_x, &data_y, &data_z, &params, beta) };

            assert!(chi2_simd.is_finite(), "amp={}: chi2 should be finite", amp);

            let rel_chi2 = chi2_simd / (n as f32 * amp * amp);
            assert!(
                rel_chi2 < 1e-10,
                "amp={}: relative chi2={} should be near zero",
                amp,
                rel_chi2
            );
        }
    }

    #[test]
    fn test_pixel_at_center() {
        if !cpu_features::has_avx2_fma() {
            println!("AVX2 not available, skipping test");
            return;
        }

        let params = [5.0f32, 5.0, 1.0, 2.0, 0.1];
        let beta = 2.5f32;

        // 8 pixels, one at center
        let data_x = vec![5.0, 4.0, 6.0, 4.0, 6.0, 4.0, 5.0, 6.0];
        let data_y = vec![5.0, 4.0, 4.0, 6.0, 6.0, 5.0, 4.0, 5.0];
        let data_z: Vec<f32> = (0..8)
            .map(|i| scalar_model(data_x[i], data_y[i], &params, beta))
            .collect();

        let mut jacobian = Vec::new();
        let mut residuals = Vec::new();
        unsafe {
            fill_jacobian_residuals_simd_fixed_beta(
                &data_x,
                &data_y,
                &data_z,
                &params,
                beta,
                &mut jacobian,
                &mut residuals,
            );
        }

        // At center (r=0): derivatives w.r.t. x0,y0 should be 0
        assert!(jacobian[0][0].abs() < 1e-6, "df/dx0 at center should be 0");
        assert!(jacobian[0][1].abs() < 1e-6, "df/dy0 at center should be 0");
        assert!(
            approx_eq(jacobian[0][2], 1.0, 1e-5, 1e-6),
            "df/damp at center should be 1"
        );
    }

    #[test]
    fn test_large_radius() {
        if !cpu_features::has_avx2_fma() {
            println!("AVX2 not available, skipping test");
            return;
        }

        let params = [50.0f32, 50.0, 1.0, 2.0, 0.1];
        let beta = 2.5f32;

        let data_x = vec![0.0, 100.0, 0.0, 100.0, 50.0, 50.0, 0.0, 100.0];
        let data_y = vec![0.0, 0.0, 100.0, 100.0, 0.0, 100.0, 50.0, 50.0];
        let data_z: Vec<f32> = (0..8)
            .map(|i| scalar_model(data_x[i], data_y[i], &params, beta))
            .collect();

        let chi2_simd =
            unsafe { compute_chi2_simd_fixed_beta(&data_x, &data_y, &data_z, &params, beta) };

        assert!(chi2_simd.is_finite(), "Chi2 should be finite");
        assert!(chi2_simd < 1e-10, "Chi2 should be near zero");
    }

    #[test]
    fn test_all_results_finite() {
        if !cpu_features::has_avx2_fma() {
            println!("AVX2 not available, skipping test");
            return;
        }

        let params = [12.0f32, 12.0, 5.0, 3.0, 0.5];
        let beta = 2.5f32;
        let (data_x, data_y, mut data_z) = generate_test_data(24, &params, beta);

        for (i, z) in data_z.iter_mut().enumerate() {
            *z += 0.1 * ((i % 3) as f32 - 1.0);
        }

        let mut jacobian = Vec::new();
        let mut residuals = Vec::new();
        unsafe {
            fill_jacobian_residuals_simd_fixed_beta(
                &data_x,
                &data_y,
                &data_z,
                &params,
                beta,
                &mut jacobian,
                &mut residuals,
            );
        }

        for (i, row) in jacobian.iter().enumerate() {
            for (j, &val) in row.iter().enumerate() {
                assert!(
                    val.is_finite(),
                    "Jacobian[{}][{}]={} is not finite",
                    i,
                    j,
                    val
                );
            }
        }

        for (i, &r) in residuals.iter().enumerate() {
            assert!(r.is_finite(), "Residual[{}]={} is not finite", i, r);
        }

        let chi2 =
            unsafe { compute_chi2_simd_fixed_beta(&data_x, &data_y, &data_z, &params, beta) };
        assert!(chi2.is_finite(), "Chi2={} is not finite", chi2);
    }

    #[test]
    fn test_buffer_reuse() {
        if !cpu_features::has_avx2_fma() {
            println!("AVX2 not available, skipping test");
            return;
        }

        let params = [8.0f32, 8.0, 1.0, 2.5, 0.1];
        let beta = 2.5f32;

        let (data_x1, data_y1, data_z1) = generate_test_data(16, &params, beta);

        let mut jacobian = Vec::new();
        let mut residuals = Vec::new();

        unsafe {
            fill_jacobian_residuals_simd_fixed_beta(
                &data_x1,
                &data_y1,
                &data_z1,
                &params,
                beta,
                &mut jacobian,
                &mut residuals,
            );
        }
        assert_eq!(jacobian.len(), 16);

        // Second call with 8 pixels
        let (data_x2, data_y2, data_z2) = generate_test_data(8, &params, beta);

        unsafe {
            fill_jacobian_residuals_simd_fixed_beta(
                &data_x2,
                &data_y2,
                &data_z2,
                &params,
                beta,
                &mut jacobian,
                &mut residuals,
            );
        }
        assert_eq!(jacobian.len(), 8);

        for i in 0..8 {
            let j_scalar = scalar_jacobian(data_x2[i], data_y2[i], &params, beta);
            for j in 0..5 {
                assert!(
                    approx_eq(jacobian[i][j], j_scalar[j], 1e-5, 1e-6),
                    "After reuse: Jacobian[{}][{}] mismatch",
                    i,
                    j
                );
            }
        }
    }
}

// ============================================================================
// SSE-specific tests
// ============================================================================

#[cfg(target_arch = "x86_64")]
mod sse_tests {
    use super::super::sse::*;
    use super::*;
    use common::cpu_features;

    #[test]
    fn test_jacobian_residuals_matches_scalar() {
        if !cpu_features::has_sse4_1() {
            println!("SSE4 not available, skipping test");
            return;
        }

        let params = [8.0f32, 8.0, 1.0, 2.5, 0.1];
        let beta = 2.5f32;
        let (data_x, data_y, data_z) = generate_test_data(12, &params, beta);

        let mut jacobian_simd = Vec::new();
        let mut residuals_simd = Vec::new();
        unsafe {
            fill_jacobian_residuals_sse_fixed_beta(
                &data_x,
                &data_y,
                &data_z,
                &params,
                beta,
                &mut jacobian_simd,
                &mut residuals_simd,
            );
        }

        for i in 0..12 {
            let j_scalar = scalar_jacobian(data_x[i], data_y[i], &params, beta);
            let model = scalar_model(data_x[i], data_y[i], &params, beta);
            let residual_scalar = data_z[i] - model;

            assert!(
                approx_eq(residuals_simd[i], residual_scalar, 1e-5, 1e-6),
                "SSE Residual mismatch at {}: simd={}, scalar={}",
                i,
                residuals_simd[i],
                residual_scalar,
            );

            for j in 0..5 {
                assert!(
                    approx_eq(jacobian_simd[i][j], j_scalar[j], 1e-5, 1e-6),
                    "SSE Jacobian[{}][{}] mismatch: simd={}, scalar={}",
                    i,
                    j,
                    jacobian_simd[i][j],
                    j_scalar[j]
                );
            }
        }
    }

    #[test]
    fn test_chi2_matches_scalar() {
        if !cpu_features::has_sse4_1() {
            println!("SSE4 not available, skipping test");
            return;
        }

        let params = [8.5f32, 8.3, 0.9, 2.2, 0.15];
        let beta = 3.0f32;
        let (data_x, data_y, mut data_z) = generate_test_data(13, &params, beta);

        for (i, z) in data_z.iter_mut().enumerate() {
            *z += 0.01 * (i as f32);
        }

        let chi2_simd =
            unsafe { compute_chi2_sse_fixed_beta(&data_x, &data_y, &data_z, &params, beta) };

        let mut chi2_scalar = 0.0f32;
        for i in 0..data_x.len() {
            let model = scalar_model(data_x[i], data_y[i], &params, beta);
            let residual = data_z[i] - model;
            chi2_scalar += residual * residual;
        }

        assert!(
            approx_eq(chi2_simd, chi2_scalar, 1e-5, 1e-6),
            "SSE Chi2 mismatch: simd={}, scalar={}",
            chi2_simd,
            chi2_scalar
        );
    }

    #[test]
    fn test_exactly_4_pixels() {
        if !cpu_features::has_sse4_1() {
            println!("SSE4 not available, skipping test");
            return;
        }

        let params = [4.0f32, 4.0, 1.0, 2.0, 0.1];
        let beta = 2.5f32;
        let (data_x, data_y, data_z) = generate_test_data(4, &params, beta);

        let mut jacobian = Vec::new();
        let mut residuals = Vec::new();
        unsafe {
            fill_jacobian_residuals_sse_fixed_beta(
                &data_x,
                &data_y,
                &data_z,
                &params,
                beta,
                &mut jacobian,
                &mut residuals,
            );
        }

        assert_eq!(jacobian.len(), 4);
        for i in 0..4 {
            let j_scalar = scalar_jacobian(data_x[i], data_y[i], &params, beta);
            for j in 0..5 {
                assert!(
                    approx_eq(jacobian[i][j], j_scalar[j], 1e-5, 1e-6),
                    "SSE Jacobian[{}][{}] mismatch",
                    i,
                    j
                );
            }
        }
    }

    #[test]
    fn test_5_pixels_simd_plus_scalar() {
        if !cpu_features::has_sse4_1() {
            println!("SSE4 not available, skipping test");
            return;
        }

        let params = [5.0f32, 5.0, 0.8, 2.5, 0.1];
        let beta = 2.5f32;
        let (data_x, data_y, data_z) = generate_test_data(5, &params, beta);

        let mut jacobian = Vec::new();
        let mut residuals = Vec::new();
        unsafe {
            fill_jacobian_residuals_sse_fixed_beta(
                &data_x,
                &data_y,
                &data_z,
                &params,
                beta,
                &mut jacobian,
                &mut residuals,
            );
        }

        assert_eq!(jacobian.len(), 5);

        // Verify the 5th element (scalar fallback)
        let j_scalar = scalar_jacobian(data_x[4], data_y[4], &params, beta);
        for j in 0..5 {
            assert!(
                approx_eq(jacobian[4][j], j_scalar[j], 1e-5, 1e-6),
                "SSE Scalar fallback Jacobian[4][{}] mismatch",
                j
            );
        }
    }

    #[test]
    fn test_handles_small_arrays() {
        if !cpu_features::has_sse4_1() {
            println!("SSE4 not available, skipping test");
            return;
        }

        // Test with fewer than 4 pixels (pure scalar fallback)
        let params = [7.0f32, 7.0, 0.7, 2.0, 0.1];
        let beta = 2.5f32;
        let (data_x, data_y, data_z) = generate_test_data(3, &params, beta);

        let mut jacobian = Vec::new();
        let mut residuals = Vec::new();
        unsafe {
            fill_jacobian_residuals_sse_fixed_beta(
                &data_x,
                &data_y,
                &data_z,
                &params,
                beta,
                &mut jacobian,
                &mut residuals,
            );
        }

        assert_eq!(jacobian.len(), 3);
        assert_eq!(residuals.len(), 3);

        let chi2 = unsafe { compute_chi2_sse_fixed_beta(&data_x, &data_y, &data_z, &params, beta) };
        assert!(chi2.is_finite());
    }

    #[test]
    fn test_empty_arrays() {
        if !cpu_features::has_sse4_1() {
            println!("SSE4 not available, skipping test");
            return;
        }

        let data_x: Vec<f32> = vec![];
        let data_y: Vec<f32> = vec![];
        let data_z: Vec<f32> = vec![];
        let params = [5.0f32, 5.0, 1.0, 2.0, 0.1];
        let beta = 2.5f32;

        let mut jacobian = Vec::new();
        let mut residuals = Vec::new();
        unsafe {
            fill_jacobian_residuals_sse_fixed_beta(
                &data_x,
                &data_y,
                &data_z,
                &params,
                beta,
                &mut jacobian,
                &mut residuals,
            );
        }

        assert_eq!(jacobian.len(), 0);
        assert_eq!(residuals.len(), 0);

        let chi2 = unsafe { compute_chi2_sse_fixed_beta(&data_x, &data_y, &data_z, &params, beta) };
        assert_eq!(chi2, 0.0);
    }

    #[test]
    fn test_various_sizes() {
        if !cpu_features::has_sse4_1() {
            println!("SSE4 not available, skipping test");
            return;
        }

        let params = [8.0f32, 8.0, 1.0, 2.5, 0.1];
        let beta = 2.5f32;

        for n in [1, 2, 3, 4, 5, 7, 8, 9, 12, 15, 16, 17, 100] {
            let (data_x, data_y, data_z) = generate_test_data(n, &params, beta);

            let chi2_sse =
                unsafe { compute_chi2_sse_fixed_beta(&data_x, &data_y, &data_z, &params, beta) };
            let chi2_scalar = compute_chi2_scalar(&data_x, &data_y, &data_z, &params, beta);

            assert!(
                approx_eq(chi2_sse, chi2_scalar, 1e-4, 1e-5),
                "n={}: SSE chi2={} vs scalar={}",
                n,
                chi2_sse,
                chi2_scalar
            );
        }
    }

    #[test]
    fn test_all_results_finite() {
        if !cpu_features::has_sse4_1() {
            println!("SSE4 not available, skipping test");
            return;
        }

        let params = [12.0f32, 12.0, 5.0, 3.0, 0.5];
        let beta = 2.5f32;
        let (data_x, data_y, mut data_z) = generate_test_data(20, &params, beta);

        for (i, z) in data_z.iter_mut().enumerate() {
            *z += 0.1 * ((i % 3) as f32 - 1.0);
        }

        let mut jacobian = Vec::new();
        let mut residuals = Vec::new();
        unsafe {
            fill_jacobian_residuals_sse_fixed_beta(
                &data_x,
                &data_y,
                &data_z,
                &params,
                beta,
                &mut jacobian,
                &mut residuals,
            );
        }

        for (i, row) in jacobian.iter().enumerate() {
            for (j, &val) in row.iter().enumerate() {
                assert!(
                    val.is_finite(),
                    "SSE Jacobian[{}][{}]={} is not finite",
                    i,
                    j,
                    val
                );
            }
        }

        for (i, &r) in residuals.iter().enumerate() {
            assert!(r.is_finite(), "SSE Residual[{}]={} is not finite", i, r);
        }

        let chi2 = unsafe { compute_chi2_sse_fixed_beta(&data_x, &data_y, &data_z, &params, beta) };
        assert!(chi2.is_finite(), "SSE Chi2={} is not finite", chi2);
    }
}
