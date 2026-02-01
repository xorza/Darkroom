//! Tests for SIMD Moffat computations.

#[cfg(target_arch = "x86_64")]
mod avx2_tests {
    use super::super::avx2::*;
    use super::super::is_avx2_available;

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

    // ========================================================================
    // Basic functionality tests
    // ========================================================================

    #[test]
    fn test_jacobian_residuals_matches_scalar() {
        if !is_avx2_available() {
            println!("AVX2 not available, skipping test");
            return;
        }

        // Create test data (16 pixels for 2 SIMD iterations)
        let n = 16;
        let mut data_x = Vec::with_capacity(n);
        let mut data_y = Vec::with_capacity(n);
        let mut data_z = Vec::with_capacity(n);

        let cx = 8.0f32;
        let cy = 8.0f32;
        let amp = 1.0f32;
        let alpha = 2.5f32;
        let beta = 2.5f32;
        let bg = 0.1f32;

        // Generate Moffat profile data
        for i in 0..n {
            let x = (i % 4) as f32 + 6.0;
            let y = (i / 4) as f32 + 6.0;
            let r2 = (x - cx).powi(2) + (y - cy).powi(2);
            let z = amp * (1.0 + r2 / (alpha * alpha)).powf(-beta) + bg;
            data_x.push(x);
            data_y.push(y);
            data_z.push(z);
        }

        let params = [cx, cy, amp, alpha, bg];

        // Compute with SIMD
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

        // Compare with scalar
        for i in 0..n {
            let j_scalar = scalar_jacobian(data_x[i], data_y[i], &params, beta);
            let model = scalar_model(data_x[i], data_y[i], &params, beta);
            let residual_scalar = data_z[i] - model;

            // Residuals should match exactly (within floating point precision)
            assert!(
                approx_eq(residuals_simd[i], residual_scalar, 1e-5, 1e-6),
                "Residual mismatch at {}: simd={}, scalar={}",
                i,
                residuals_simd[i],
                residual_scalar,
            );

            // Jacobian elements should match exactly
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
        if !is_avx2_available() {
            println!("AVX2 not available, skipping test");
            return;
        }

        let n = 17; // Non-multiple of 8 to test scalar fallback
        let mut data_x = Vec::with_capacity(n);
        let mut data_y = Vec::with_capacity(n);
        let mut data_z = Vec::with_capacity(n);

        let cx = 8.5f32;
        let cy = 8.3f32;
        let amp = 0.9f32;
        let alpha = 2.2f32;
        let beta = 3.0f32;
        let bg = 0.15f32;

        // Generate noisy Moffat profile data
        for i in 0..n {
            let x = (i % 5) as f32 + 6.0;
            let y = (i / 5) as f32 + 6.0;
            let r2 = (x - cx).powi(2) + (y - cy).powi(2);
            let z = amp * (1.0 + r2 / (alpha * alpha)).powf(-beta) + bg + 0.01 * (i as f32);
            data_x.push(x);
            data_y.push(y);
            data_z.push(z);
        }

        let params = [cx, cy, amp, alpha, bg];

        // Compute with SIMD
        let chi2_simd =
            unsafe { compute_chi2_simd_fixed_beta(&data_x, &data_y, &data_z, &params, beta) };

        // Compute with scalar
        let mut chi2_scalar = 0.0f32;
        for i in 0..n {
            let model = scalar_model(data_x[i], data_y[i], &params, beta);
            let residual = data_z[i] - model;
            chi2_scalar += residual * residual;
        }

        // Should match within floating point precision
        assert!(
            approx_eq(chi2_simd, chi2_scalar, 1e-5, 1e-6),
            "Chi2 mismatch: simd={}, scalar={}",
            chi2_simd,
            chi2_scalar
        );
    }

    // ========================================================================
    // Edge cases: array sizes
    // ========================================================================

    #[test]
    fn test_handles_small_arrays() {
        if !is_avx2_available() {
            println!("AVX2 not available, skipping test");
            return;
        }

        // Test with fewer than 8 pixels (pure scalar fallback)
        let data_x = vec![5.0, 6.0, 7.0, 8.0, 9.0];
        let data_y = vec![5.0, 6.0, 7.0, 8.0, 9.0];
        let data_z = vec![0.5, 0.6, 0.7, 0.6, 0.5];
        let params = [7.0f32, 7.0, 0.7, 2.0, 0.1];
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

        assert_eq!(jacobian.len(), 5);
        assert_eq!(residuals.len(), 5);

        let chi2 =
            unsafe { compute_chi2_simd_fixed_beta(&data_x, &data_y, &data_z, &params, beta) };
        assert!(chi2.is_finite());
    }

    #[test]
    fn test_exactly_8_pixels() {
        if !is_avx2_available() {
            println!("AVX2 not available, skipping test");
            return;
        }

        // Exactly 8 pixels - one full SIMD iteration, no scalar fallback
        let n = 8;
        let params = [4.0f32, 4.0, 1.0, 2.0, 0.1];
        let beta = 2.5f32;

        let mut data_x = Vec::with_capacity(n);
        let mut data_y = Vec::with_capacity(n);
        let mut data_z = Vec::with_capacity(n);

        for i in 0..n {
            let x = (i % 3) as f32 + 3.0;
            let y = (i / 3) as f32 + 3.0;
            data_x.push(x);
            data_y.push(y);
            data_z.push(scalar_model(x, y, &params, beta) + 0.01);
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

        assert_eq!(jacobian.len(), 8);
        assert_eq!(residuals.len(), 8);

        // Verify each element
        for i in 0..n {
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
    fn test_exactly_16_pixels() {
        if !is_avx2_available() {
            println!("AVX2 not available, skipping test");
            return;
        }

        // Exactly 16 pixels - two full SIMD iterations
        let n = 16;
        let params = [8.0f32, 8.0, 1.0, 3.0, 0.05];
        let beta = 3.0f32;

        let mut data_x = Vec::with_capacity(n);
        let mut data_y = Vec::with_capacity(n);
        let mut data_z = Vec::with_capacity(n);

        for i in 0..n {
            let x = (i % 4) as f32 + 6.0;
            let y = (i / 4) as f32 + 6.0;
            data_x.push(x);
            data_y.push(y);
            data_z.push(scalar_model(x, y, &params, beta));
        }

        let chi2_simd =
            unsafe { compute_chi2_simd_fixed_beta(&data_x, &data_y, &data_z, &params, beta) };

        // Perfect fit should give chi2 ≈ 0
        assert!(
            chi2_simd < 1e-10,
            "Chi2 should be near zero for perfect fit"
        );
    }

    #[test]
    fn test_9_pixels_simd_plus_scalar() {
        if !is_avx2_available() {
            println!("AVX2 not available, skipping test");
            return;
        }

        // 9 pixels = 8 SIMD + 1 scalar
        let n = 9;
        let params = [5.0f32, 5.0, 0.8, 2.5, 0.1];
        let beta = 2.5f32;

        let mut data_x = Vec::with_capacity(n);
        let mut data_y = Vec::with_capacity(n);
        let mut data_z = Vec::with_capacity(n);

        for i in 0..n {
            let x = (i % 3) as f32 + 4.0;
            let y = (i / 3) as f32 + 4.0;
            data_x.push(x);
            data_y.push(y);
            data_z.push(scalar_model(x, y, &params, beta) + 0.02 * (i as f32));
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

        assert_eq!(jacobian.len(), 9);

        // Verify the 9th element (scalar fallback)
        let j_scalar = scalar_jacobian(data_x[8], data_y[8], &params, beta);
        for j in 0..5 {
            assert!(
                approx_eq(jacobian[8][j], j_scalar[j], 1e-5, 1e-6),
                "Scalar fallback Jacobian[8][{}] mismatch: simd={}, scalar={}",
                j,
                jacobian[8][j],
                j_scalar[j]
            );
        }
    }

    #[test]
    fn test_empty_arrays() {
        if !is_avx2_available() {
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

    // ========================================================================
    // Parameter variations
    // ========================================================================

    #[test]
    fn test_various_beta_values() {
        if !is_avx2_available() {
            println!("AVX2 not available, skipping test");
            return;
        }

        let n = 16;
        let params = [8.0f32, 8.0, 1.0, 2.5, 0.1];

        let mut data_x = Vec::with_capacity(n);
        let mut data_y = Vec::with_capacity(n);

        for i in 0..n {
            data_x.push((i % 4) as f32 + 6.0);
            data_y.push((i / 4) as f32 + 6.0);
        }

        // Test different beta values
        for beta in [1.5f32, 2.0, 2.5, 3.0, 4.0, 5.0] {
            let data_z: Vec<f32> = (0..n)
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

            // For perfect data, residuals should be near zero
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
        if !is_avx2_available() {
            println!("AVX2 not available, skipping test");
            return;
        }

        let n = 16;
        let beta = 2.5f32;

        let mut data_x = Vec::with_capacity(n);
        let mut data_y = Vec::with_capacity(n);

        for i in 0..n {
            data_x.push((i % 4) as f32 + 6.0);
            data_y.push((i / 4) as f32 + 6.0);
        }

        // Test different alpha values (narrow to wide PSFs)
        for alpha in [0.5f32, 1.0, 2.0, 3.0, 5.0, 10.0] {
            let params = [8.0f32, 8.0, 1.0, alpha, 0.1];
            let data_z: Vec<f32> = (0..n)
                .map(|i| scalar_model(data_x[i], data_y[i], &params, beta))
                .collect();

            let chi2_simd =
                unsafe { compute_chi2_simd_fixed_beta(&data_x, &data_y, &data_z, &params, beta) };

            // Perfect fit
            assert!(
                chi2_simd < 1e-10,
                "alpha={}: chi2={} should be near zero",
                alpha,
                chi2_simd
            );
        }
    }

    #[test]
    fn test_off_center_positions() {
        if !is_avx2_available() {
            println!("AVX2 not available, skipping test");
            return;
        }

        let n = 16;
        let beta = 2.5f32;

        let mut data_x = Vec::with_capacity(n);
        let mut data_y = Vec::with_capacity(n);

        for i in 0..n {
            data_x.push((i % 4) as f32 + 6.0);
            data_y.push((i / 4) as f32 + 6.0);
        }

        // Test various center positions (including subpixel)
        for (cx, cy) in [
            (8.0f32, 8.0f32),
            (7.3, 8.7),
            (6.0, 6.0),
            (9.9, 9.9),
            (7.123, 8.456),
        ] {
            let params = [cx, cy, 1.0, 2.5, 0.1];
            let data_z: Vec<f32> = (0..n)
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

            // Verify against scalar
            for i in 0..n {
                let j_scalar = scalar_jacobian(data_x[i], data_y[i], &params, beta);
                for j in 0..5 {
                    assert!(
                        approx_eq(jacobian[i][j], j_scalar[j], 1e-5, 1e-6),
                        "center=({},{}): Jacobian[{}][{}] mismatch: simd={}, scalar={}",
                        cx,
                        cy,
                        i,
                        j,
                        jacobian[i][j],
                        j_scalar[j]
                    );
                }
            }
        }
    }

    #[test]
    fn test_extreme_amplitude_values() {
        if !is_avx2_available() {
            println!("AVX2 not available, skipping test");
            return;
        }

        let n = 16;
        let beta = 2.5f32;

        let mut data_x = Vec::with_capacity(n);
        let mut data_y = Vec::with_capacity(n);

        for i in 0..n {
            data_x.push((i % 4) as f32 + 6.0);
            data_y.push((i / 4) as f32 + 6.0);
        }

        // Test extreme amplitude values
        for amp in [0.001f32, 0.1, 1.0, 10.0, 1000.0, 65535.0] {
            let params = [8.0f32, 8.0, amp, 2.5, 0.1];
            let data_z: Vec<f32> = (0..n)
                .map(|i| scalar_model(data_x[i], data_y[i], &params, beta))
                .collect();

            let chi2_simd =
                unsafe { compute_chi2_simd_fixed_beta(&data_x, &data_y, &data_z, &params, beta) };

            assert!(chi2_simd.is_finite(), "amp={}: chi2 should be finite", amp);

            // For large amplitudes, use relative tolerance
            // chi2 / (n * amp^2) should be small
            let rel_chi2 = chi2_simd / (n as f32 * amp * amp);
            assert!(
                rel_chi2 < 1e-10,
                "amp={}: relative chi2={} should be near zero for perfect fit",
                amp,
                rel_chi2
            );
        }
    }

    // ========================================================================
    // Numerical stability tests
    // ========================================================================

    #[test]
    fn test_pixel_at_center() {
        if !is_avx2_available() {
            println!("AVX2 not available, skipping test");
            return;
        }

        // Test when pixel is exactly at center (r=0)
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

        // At center (r=0): u=1, u^(-beta)=1, derivatives w.r.t. x0,y0 should be 0
        assert!(jacobian[0][0].abs() < 1e-6, "df/dx0 at center should be 0");
        assert!(jacobian[0][1].abs() < 1e-6, "df/dy0 at center should be 0");
        assert!(
            approx_eq(jacobian[0][2], 1.0, 1e-5, 1e-6),
            "df/damp at center should be 1"
        );
    }

    #[test]
    fn test_large_radius() {
        if !is_avx2_available() {
            println!("AVX2 not available, skipping test");
            return;
        }

        // Test pixels far from center
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
        assert!(
            chi2_simd < 1e-10,
            "Chi2 should be near zero for perfect fit"
        );
    }

    #[test]
    fn test_all_results_finite() {
        if !is_avx2_available() {
            println!("AVX2 not available, skipping test");
            return;
        }

        // Stress test with many random-ish parameters
        let n = 24; // 3 SIMD iterations
        let params = [12.0f32, 12.0, 5.0, 3.0, 0.5];
        let beta = 2.5f32;

        let mut data_x = Vec::with_capacity(n);
        let mut data_y = Vec::with_capacity(n);
        let mut data_z = Vec::with_capacity(n);

        for i in 0..n {
            let x = (i % 5) as f32 * 2.0 + 5.0;
            let y = (i / 5) as f32 * 2.0 + 5.0;
            data_x.push(x);
            data_y.push(y);
            data_z.push(scalar_model(x, y, &params, beta) + 0.1 * ((i % 3) as f32 - 1.0));
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

        // All values should be finite
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

    // ========================================================================
    // Buffer reuse tests
    // ========================================================================

    #[test]
    fn test_buffer_reuse() {
        if !is_avx2_available() {
            println!("AVX2 not available, skipping test");
            return;
        }

        let params = [8.0f32, 8.0, 1.0, 2.5, 0.1];
        let beta = 2.5f32;

        // First call with 16 pixels
        let data_x1: Vec<f32> = (0..16).map(|i| (i % 4) as f32 + 6.0).collect();
        let data_y1: Vec<f32> = (0..16).map(|i| (i / 4) as f32 + 6.0).collect();
        let data_z1: Vec<f32> = (0..16)
            .map(|i| scalar_model(data_x1[i], data_y1[i], &params, beta))
            .collect();

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
        assert_eq!(residuals.len(), 16);

        // Second call with 8 pixels - buffers should be resized
        let data_x2: Vec<f32> = (0..8).map(|i| (i % 3) as f32 + 7.0).collect();
        let data_y2: Vec<f32> = (0..8).map(|i| (i / 3) as f32 + 7.0).collect();
        let data_z2: Vec<f32> = (0..8)
            .map(|i| scalar_model(data_x2[i], data_y2[i], &params, beta))
            .collect();

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
        assert_eq!(residuals.len(), 8);

        // Verify correctness after reuse
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
