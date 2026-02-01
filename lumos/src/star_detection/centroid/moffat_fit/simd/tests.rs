//! Tests for SIMD Moffat computations.

#[cfg(target_arch = "x86_64")]
mod avx2_tests {
    use super::super::avx2::*;
    use super::super::is_avx2_available;

    fn approx_eq(a: f32, b: f32, rel_tolerance: f32, abs_tolerance: f32) -> bool {
        let diff = (a - b).abs();
        diff < abs_tolerance || diff / a.abs().max(b.abs()).max(1e-10) < rel_tolerance
    }

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

        // Compute with scalar
        let alpha2 = alpha * alpha;
        for i in 0..n {
            let x = data_x[i];
            let y = data_y[i];
            let z = data_z[i];

            let dx = x - cx;
            let dy = y - cy;
            let r2 = dx * dx + dy * dy;
            let u = 1.0 + r2 / alpha2;
            let u_neg_beta = u.powf(-beta);
            let u_neg_beta_m1 = u_neg_beta / u;
            let model = amp * u_neg_beta + bg;
            let residual_scalar = z - model;

            let common = 2.0 * amp * beta / alpha2 * u_neg_beta_m1;
            let j_scalar = [
                common * dx,
                common * dy,
                u_neg_beta,
                common * r2 / alpha,
                1.0,
            ];

            // Residuals should match exactly (within floating point precision)
            assert!(
                approx_eq(residuals_simd[i], residual_scalar, 1e-5, 1e-6),
                "Residual mismatch at {}: simd={}, scalar={}",
                i,
                residuals_simd[i],
                residual_scalar,
            );

            // Jacobian elements should match exactly (within floating point precision)
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
        let alpha2 = alpha * alpha;
        let mut chi2_scalar = 0.0f32;
        for i in 0..n {
            let x = data_x[i];
            let y = data_y[i];
            let z = data_z[i];

            let dx = x - cx;
            let dy = y - cy;
            let r2 = dx * dx + dy * dy;
            let u = 1.0 + r2 / alpha2;
            let model = amp * u.powf(-beta) + bg;
            let residual = z - model;
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
}
