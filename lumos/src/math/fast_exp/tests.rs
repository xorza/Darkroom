//! Tests for vectorized fast exp implementations.

use super::*;

/// Max acceptable relative error for the fast exp (2 ULP for f32).
const MAX_REL_ERROR: f32 = 3e-7;

/// Scalar fast exp used only for testing. Uses the same polynomial
/// as the SIMD versions for consistency verification.
fn fast_exp_scalar(x: f32) -> f32 {
    let x = x.clamp(EXP_LO, EXP_HI);

    let n = (x * LOG2E + 0.5).floor();
    let r = x - n * LN2_HI - n * LN2_LO;

    let p = EXP_C6;
    let p = p * r + EXP_C5;
    let p = p * r + EXP_C4;
    let p = p * r + EXP_C3;
    let p = p * r + EXP_C2;
    let p = p * r + EXP_C1;
    let p = p * r + 1.0;

    let n_i = n as i32;
    let pow2n = f32::from_bits(((127 + n_i) as u32) << 23);
    p * pow2n
}

/// Helper: compute relative error, handling near-zero values.
fn rel_error(actual: f32, expected: f32) -> f32 {
    let diff = (actual - expected).abs();
    if expected.abs() < 1e-30 {
        diff
    } else {
        diff / expected.abs()
    }
}

#[test]
fn test_fast_exp_scalar_accuracy() {
    // Test a range of inputs from -87 to 20
    let test_values: Vec<f32> = (-870..=200).map(|i| i as f32 * 0.1).collect();

    let mut max_err = 0.0f32;
    for &x in &test_values {
        let fast = fast_exp_scalar(x);
        let exact = x.exp();
        let err = rel_error(fast, exact);
        max_err = max_err.max(err);
        assert!(
            err < MAX_REL_ERROR,
            "fast_exp_scalar({}) = {}, expected {}, rel_error = {:.2e}",
            x,
            fast,
            exact,
            err
        );
    }
    println!("Scalar max relative error: {:.2e}", max_err);
}

#[test]
fn test_fast_exp_scalar_special_values() {
    // exp(0) = 1
    let result = fast_exp_scalar(0.0);
    assert!(
        (result - 1.0).abs() < 1e-6,
        "exp(0) = {}, expected 1.0",
        result
    );

    // Very negative: should be near 0
    let result = fast_exp_scalar(-87.0);
    assert!(result > 0.0, "exp(-87) should be positive, got {}", result);
    assert!(result < 1e-37, "exp(-87) should be tiny, got {}", result);

    // Large positive: should be large
    let result = fast_exp_scalar(20.0);
    let exact = 20.0f32.exp();
    let err = rel_error(result, exact);
    assert!(err < MAX_REL_ERROR, "exp(20) rel_error = {:.2e}", err);
}

#[test]
fn test_fast_exp_scalar_negative_range() {
    // Gaussian exponents are always negative: -0.5 * (d²/σ²)
    // Typical range: -20 to 0
    for i in -200..=0 {
        let x = i as f32 * 0.1;
        let fast = fast_exp_scalar(x);
        let exact = x.exp();
        let err = rel_error(fast, exact);
        assert!(
            err < MAX_REL_ERROR,
            "fast_exp_scalar({}) = {}, expected {}, rel_error = {:.2e}",
            x,
            fast,
            exact,
            err
        );
    }
}

#[cfg(target_arch = "x86_64")]
mod avx2_tests {
    use super::*;
    use crate::math::fast_exp::fast_exp_8_avx2;
    use common::cpu_features;

    #[test]
    fn test_fast_exp_8_accuracy() {
        if !cpu_features::has_avx2_fma() {
            println!("AVX2 not available, skipping");
            return;
        }

        let mut max_err = 0.0f32;
        // Test in batches of 8
        let inputs: Vec<f32> = (-870..=200).map(|i| i as f32 * 0.1).collect();

        for chunk in inputs.chunks(8) {
            let mut batch = [0.0f32; 8];
            for (i, &v) in chunk.iter().enumerate() {
                batch[i] = v;
            }

            let result = unsafe { fast_exp_8_avx2(&batch) };

            for i in 0..chunk.len() {
                let exact = chunk[i].exp();
                let err = rel_error(result[i], exact);
                max_err = max_err.max(err);
                assert!(
                    err < MAX_REL_ERROR,
                    "fast_exp_8_avx2: x={}, got={}, expected={}, rel_error={:.2e}",
                    chunk[i],
                    result[i],
                    exact,
                    err
                );
            }
        }
        println!("AVX2 max relative error: {:.2e}", max_err);
    }

    #[test]
    fn test_fast_exp_8_matches_scalar() {
        if !cpu_features::has_avx2_fma() {
            println!("AVX2 not available, skipping");
            return;
        }

        let batch = [-10.0f32, -5.0, -2.0, -1.0, -0.5, 0.0, 1.0, 5.0];
        let result = unsafe { fast_exp_8_avx2(&batch) };

        for i in 0..8 {
            let scalar = fast_exp_scalar(batch[i]);
            let diff = (result[i] - scalar).abs();
            // AVX2 uses FMA so may differ slightly from scalar (which uses mul+add)
            assert!(
                diff / scalar.abs().max(1e-30) < 1e-6,
                "AVX2 vs scalar mismatch at x={}: avx2={}, scalar={}",
                batch[i],
                result[i],
                scalar
            );
        }
    }

    #[test]
    fn test_fast_exp_8_gaussian_range() {
        if !cpu_features::has_avx2_fma() {
            println!("AVX2 not available, skipping");
            return;
        }

        // Typical Gaussian exponents: -0.5 * dist²/sigma²
        // For sigma=2, stamp_radius=8: max dist²=128, exponent=-16
        let batch = [-16.0f32, -12.0, -8.0, -4.0, -2.0, -1.0, -0.5, -0.1];
        let result = unsafe { fast_exp_8_avx2(&batch) };

        for i in 0..8 {
            let exact = batch[i].exp();
            let err = rel_error(result[i], exact);
            assert!(
                err < MAX_REL_ERROR,
                "Gaussian range: x={}, got={}, expected={}, rel_error={:.2e}",
                batch[i],
                result[i],
                exact,
                err
            );
        }
    }

    #[test]
    fn test_fast_exp_8_clamping() {
        if !cpu_features::has_avx2_fma() {
            println!("AVX2 not available, skipping");
            return;
        }

        // Values beyond f32 exp range should be clamped, not produce NaN/Inf
        let batch = [-200.0f32, -100.0, -88.0, -87.0, 88.0, 89.0, 100.0, 200.0];
        let result = unsafe { fast_exp_8_avx2(&batch) };

        for i in 0..8 {
            assert!(
                result[i].is_finite(),
                "fast_exp_8 should produce finite results, got {} for x={}",
                result[i],
                batch[i]
            );
            assert!(
                result[i] >= 0.0,
                "fast_exp_8 should be non-negative, got {} for x={}",
                result[i],
                batch[i]
            );
        }
    }
}

#[cfg(target_arch = "x86_64")]
mod sse_tests {
    use super::*;
    use crate::math::fast_exp::fast_exp_4_sse;
    use common::cpu_features;

    #[test]
    fn test_fast_exp_4_accuracy() {
        if !cpu_features::has_sse4_1() {
            println!("SSE4.1 not available, skipping");
            return;
        }

        let mut max_err = 0.0f32;
        let inputs: Vec<f32> = (-870..=200).map(|i| i as f32 * 0.1).collect();

        for chunk in inputs.chunks(4) {
            let mut batch = [0.0f32; 4];
            for (i, &v) in chunk.iter().enumerate() {
                batch[i] = v;
            }

            let result = unsafe { fast_exp_4_sse(&batch) };

            for i in 0..chunk.len() {
                let exact = chunk[i].exp();
                let err = rel_error(result[i], exact);
                max_err = max_err.max(err);
                assert!(
                    err < MAX_REL_ERROR,
                    "fast_exp_4_sse: x={}, got={}, expected={}, rel_error={:.2e}",
                    chunk[i],
                    result[i],
                    exact,
                    err
                );
            }
        }
        println!("SSE max relative error: {:.2e}", max_err);
    }

    #[test]
    fn test_fast_exp_4_clamping() {
        if !cpu_features::has_sse4_1() {
            println!("SSE4.1 not available, skipping");
            return;
        }

        let batch = [-200.0f32, -100.0, 100.0, 200.0];
        let result = unsafe { fast_exp_4_sse(&batch) };

        for i in 0..4 {
            assert!(
                result[i].is_finite(),
                "fast_exp_4 should produce finite results, got {} for x={}",
                result[i],
                batch[i]
            );
            assert!(
                result[i] >= 0.0,
                "fast_exp_4 should be non-negative, got {} for x={}",
                result[i],
                batch[i]
            );
        }
    }
}
