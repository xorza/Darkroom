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
    use crate::math::fast_exp::sse::fast_exp_4_sse_m128;
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
    fn test_fast_exp_4_matches_scalar() {
        if !cpu_features::has_sse4_1() {
            println!("SSE4.1 not available, skipping");
            return;
        }

        let batch = [-10.0f32, -5.0, -1.0, 0.0];
        let result = unsafe { fast_exp_4_sse(&batch) };

        for i in 0..4 {
            let scalar = fast_exp_scalar(batch[i]);
            let diff = (result[i] - scalar).abs();
            // SSE uses mul+add (no FMA) so may differ very slightly from scalar
            assert!(
                diff / scalar.abs().max(1e-30) < 1e-6,
                "SSE vs scalar mismatch at x={}: sse={}, scalar={}",
                batch[i],
                result[i],
                scalar
            );
        }
    }

    #[test]
    fn test_fast_exp_4_gaussian_range() {
        if !cpu_features::has_sse4_1() {
            println!("SSE4.1 not available, skipping");
            return;
        }

        let batch = [-16.0f32, -8.0, -2.0, -0.1];
        let result = unsafe { fast_exp_4_sse(&batch) };

        for i in 0..4 {
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

    #[test]
    fn test_fast_exp_4_m128_matches_array() {
        if !cpu_features::has_sse4_1() {
            println!("SSE4.1 not available, skipping");
            return;
        }

        use std::arch::x86_64::*;

        let batch = [-12.5f32, -3.7, -0.01, 15.0];
        let array_result = unsafe { fast_exp_4_sse(&batch) };

        let m128_result = unsafe {
            let vx = _mm_loadu_ps(batch.as_ptr());
            let vr = fast_exp_4_sse_m128(vx);
            let mut out = [0.0f32; 4];
            _mm_storeu_ps(out.as_mut_ptr(), vr);
            out
        };

        for i in 0..4 {
            assert_eq!(
                array_result[i], m128_result[i],
                "m128 vs array mismatch at x={}: m128={}, array={}",
                batch[i], m128_result[i], array_result[i]
            );
        }
    }

    #[test]
    fn test_fast_exp_4_monotonicity() {
        if !cpu_features::has_sse4_1() {
            println!("SSE4.1 not available, skipping");
            return;
        }

        // exp is monotonically increasing: if x1 < x2 then exp(x1) <= exp(x2)
        let inputs: Vec<f32> = (-870..=200).map(|i| i as f32 * 0.1).collect();

        for chunk in inputs.windows(4) {
            let batch_lo = [chunk[0], chunk[0], chunk[0], chunk[0]];
            let batch_hi = [chunk[1], chunk[2], chunk[3], chunk[3]];
            let result_lo = unsafe { fast_exp_4_sse(&batch_lo) };
            let result_hi = unsafe { fast_exp_4_sse(&batch_hi) };
            // Verify first element pairs maintain monotonicity
            assert!(
                result_lo[0] <= result_hi[0],
                "SSE monotonicity violated: exp({}) = {} > exp({}) = {}",
                chunk[0],
                result_lo[0],
                chunk[1],
                result_hi[0]
            );
        }
    }
}

// ============================================================================
// Cross-implementation consistency tests
// ============================================================================

#[cfg(target_arch = "x86_64")]
mod cross_impl_tests {
    use super::*;
    use crate::math::fast_exp::avx2::fast_exp_8_avx2_m256;
    use crate::math::fast_exp::fast_exp_4_sse;
    use crate::math::fast_exp::fast_exp_8_avx2;
    use crate::math::fast_exp::sse::fast_exp_4_sse_m128;
    use common::cpu_features;

    #[test]
    fn test_avx2_m256_matches_array() {
        if !cpu_features::has_avx2_fma() {
            println!("AVX2 not available, skipping");
            return;
        }

        use std::arch::x86_64::*;

        let batch = [-20.0f32, -10.0, -5.0, -1.0, 0.0, 3.0, 10.0, 18.0];
        let array_result = unsafe { fast_exp_8_avx2(&batch) };

        let m256_result = unsafe {
            let vx = _mm256_loadu_ps(batch.as_ptr());
            let vr = fast_exp_8_avx2_m256(vx);
            let mut out = [0.0f32; 8];
            _mm256_storeu_ps(out.as_mut_ptr(), vr);
            out
        };

        for i in 0..8 {
            assert_eq!(
                array_result[i], m256_result[i],
                "m256 vs array mismatch at x={}: m256={}, array={}",
                batch[i], m256_result[i], array_result[i]
            );
        }
    }

    #[test]
    fn test_avx2_sse_consistency() {
        if !cpu_features::has_avx2_fma() || !cpu_features::has_sse4_1() {
            println!("AVX2+SSE not available, skipping");
            return;
        }

        // Both should produce results within tolerance of each other
        // Note: AVX2 uses FMA while SSE uses mul+add, so minor differences expected
        let inputs: Vec<f32> = (-200..=200).map(|i| i as f32 * 0.1).collect();

        for chunk in inputs.chunks(8) {
            if chunk.len() < 8 {
                continue;
            }
            let mut batch8 = [0.0f32; 8];
            batch8.copy_from_slice(chunk);
            let avx_result = unsafe { fast_exp_8_avx2(&batch8) };

            let batch4_lo: [f32; 4] = [chunk[0], chunk[1], chunk[2], chunk[3]];
            let batch4_hi: [f32; 4] = [chunk[4], chunk[5], chunk[6], chunk[7]];
            let sse_lo = unsafe { fast_exp_4_sse(&batch4_lo) };
            let sse_hi = unsafe { fast_exp_4_sse(&batch4_hi) };

            for i in 0..4 {
                let diff = (avx_result[i] - sse_lo[i]).abs();
                let max_val = avx_result[i].abs().max(sse_lo[i].abs()).max(1e-30);
                assert!(
                    diff / max_val < 1e-6,
                    "AVX2 vs SSE mismatch at x={}: avx2={}, sse={}",
                    chunk[i],
                    avx_result[i],
                    sse_lo[i]
                );
            }
            for i in 0..4 {
                let diff = (avx_result[i + 4] - sse_hi[i]).abs();
                let max_val = avx_result[i + 4].abs().max(sse_hi[i].abs()).max(1e-30);
                assert!(
                    diff / max_val < 1e-6,
                    "AVX2 vs SSE mismatch at x={}: avx2={}, sse={}",
                    chunk[i + 4],
                    avx_result[i + 4],
                    sse_hi[i]
                );
            }
        }
    }

    #[test]
    fn test_avx2_monotonicity() {
        if !cpu_features::has_avx2_fma() {
            println!("AVX2 not available, skipping");
            return;
        }

        // Test monotonicity with fine-grained steps
        let inputs: Vec<f32> = (-8700..=880).map(|i| i as f32 * 0.01).collect();

        for pair in inputs.windows(2) {
            let batch_lo = [pair[0]; 8];
            let batch_hi = [pair[1]; 8];
            let result_lo = unsafe { fast_exp_8_avx2(&batch_lo) };
            let result_hi = unsafe { fast_exp_8_avx2(&batch_hi) };
            assert!(
                result_lo[0] <= result_hi[0],
                "AVX2 monotonicity violated: exp({}) = {} > exp({}) = {}",
                pair[0],
                result_lo[0],
                pair[1],
                result_hi[0]
            );
        }
    }

    #[test]
    fn test_boundary_values() {
        if !cpu_features::has_avx2_fma() || !cpu_features::has_sse4_1() {
            println!("AVX2+SSE not available, skipping");
            return;
        }

        // Test at exact clamping boundaries and polynomial approximation edges
        let ln2_half = std::f32::consts::LN_2 / 2.0;
        let boundary_values = [
            EXP_LO,            // lower clamp boundary
            EXP_LO + 0.001,    // just above lower clamp
            EXP_HI - 0.001,    // just below upper clamp
            EXP_HI,            // upper clamp boundary
            -ln2_half,         // polynomial domain edge (negative)
            ln2_half,          // polynomial domain edge (positive)
            -ln2_half + 0.001, // just inside polynomial domain
            ln2_half - 0.001,  // just inside polynomial domain
        ];

        // AVX2
        let avx_result = unsafe { fast_exp_8_avx2(&boundary_values) };
        for i in 0..8 {
            assert!(
                avx_result[i].is_finite(),
                "AVX2 boundary: x={} produced non-finite {}",
                boundary_values[i],
                avx_result[i]
            );
            assert!(
                avx_result[i] >= 0.0,
                "AVX2 boundary: x={} produced negative {}",
                boundary_values[i],
                avx_result[i]
            );
            // For values within valid exp range, check accuracy
            let exact = boundary_values[i].exp();
            if exact.is_finite() && exact > 0.0 {
                let err = rel_error(avx_result[i], exact);
                assert!(
                    err < MAX_REL_ERROR,
                    "AVX2 boundary accuracy: x={}, got={}, expected={}, rel_error={:.2e}",
                    boundary_values[i],
                    avx_result[i],
                    exact,
                    err
                );
            }
        }

        // SSE: test first 4 and last 4
        let batch_lo: [f32; 4] = boundary_values[..4].try_into().unwrap();
        let batch_hi: [f32; 4] = boundary_values[4..].try_into().unwrap();
        let sse_lo = unsafe { fast_exp_4_sse(&batch_lo) };
        let sse_hi = unsafe { fast_exp_4_sse(&batch_hi) };

        for (i, &result) in sse_lo.iter().chain(sse_hi.iter()).enumerate() {
            assert!(
                result.is_finite(),
                "SSE boundary: x={} produced non-finite {}",
                boundary_values[i],
                result
            );
            assert!(
                result >= 0.0,
                "SSE boundary: x={} produced negative {}",
                boundary_values[i],
                result
            );
        }
    }

    #[test]
    fn test_special_float_inputs() {
        if !cpu_features::has_avx2_fma() || !cpu_features::has_sse4_1() {
            println!("AVX2+SSE not available, skipping");
            return;
        }

        // NaN, +Inf, -Inf, -0.0, subnormal
        let special = [
            f32::NAN,
            f32::INFINITY,
            f32::NEG_INFINITY,
            -0.0f32,
            f32::MIN_POSITIVE,        // smallest normal
            f32::MIN_POSITIVE / 2.0,  // subnormal
            -f32::MIN_POSITIVE,       // negative smallest normal
            -f32::MIN_POSITIVE / 2.0, // negative subnormal
        ];

        let avx_result = unsafe { fast_exp_8_avx2(&special) };

        // exp(-0) = 1
        assert!(
            (avx_result[3] - 1.0).abs() < 1e-6,
            "exp(-0) should be 1.0, got {}",
            avx_result[3]
        );

        // exp(tiny positive) ≈ 1
        assert!(
            (avx_result[4] - 1.0).abs() < 1e-6,
            "exp(MIN_POSITIVE) should be ~1.0, got {}",
            avx_result[4]
        );
        assert!(
            (avx_result[5] - 1.0).abs() < 1e-6,
            "exp(subnormal) should be ~1.0, got {}",
            avx_result[5]
        );

        // exp(tiny negative) ≈ 1
        assert!(
            (avx_result[6] - 1.0).abs() < 1e-6,
            "exp(-MIN_POSITIVE) should be ~1.0, got {}",
            avx_result[6]
        );
        assert!(
            (avx_result[7] - 1.0).abs() < 1e-6,
            "exp(-subnormal) should be ~1.0, got {}",
            avx_result[7]
        );

        // exp(-Inf) is clamped (produces ~0)
        assert!(
            avx_result[2].is_finite(),
            "exp(-Inf) should be clamped to finite, got {}",
            avx_result[2]
        );
        assert!(
            avx_result[2] >= 0.0,
            "exp(-Inf) should be non-negative, got {}",
            avx_result[2]
        );

        // exp(+Inf) is clamped to a large finite value
        assert!(
            avx_result[1].is_finite(),
            "exp(+Inf) should be clamped to finite, got {}",
            avx_result[1]
        );

        // SSE: test same values in two batches
        let sse_lo = unsafe { fast_exp_4_sse(&[special[0], special[1], special[2], special[3]]) };
        let sse_hi = unsafe { fast_exp_4_sse(&[special[4], special[5], special[6], special[7]]) };

        // exp(-0) = 1
        assert!(
            (sse_lo[3] - 1.0).abs() < 1e-6,
            "SSE exp(-0) should be 1.0, got {}",
            sse_lo[3]
        );
        // exp(-Inf) clamped
        assert!(
            sse_lo[2].is_finite() && sse_lo[2] >= 0.0,
            "SSE exp(-Inf) should be finite non-negative, got {}",
            sse_lo[2]
        );
        // exp(+Inf) clamped
        assert!(
            sse_lo[1].is_finite(),
            "SSE exp(+Inf) should be clamped to finite, got {}",
            sse_lo[1]
        );
        // tiny values ≈ 1
        for i in 0..4 {
            assert!(
                (sse_hi[i] - 1.0).abs() < 1e-6,
                "SSE exp(tiny) should be ~1.0, got {} for input {}",
                sse_hi[i],
                special[i + 4]
            );
        }
    }
}
