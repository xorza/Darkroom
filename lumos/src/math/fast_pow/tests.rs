use super::fast_pow_neg_beta;

/// Reference implementation using powf for comparison.
fn reference_pow(u: f32, neg_beta: f32) -> f32 {
    u.powf(neg_beta)
}

#[test]
fn test_half_integer_betas_match_powf() {
    let test_u_values = [1.001, 1.1, 1.5, 2.0, 3.456, 5.0, 10.0, 50.0, 100.0];
    let half_integer_betas = [1.0, 1.5, 2.0, 2.5, 3.0, 3.5, 4.0, 4.5];

    for &beta in &half_integer_betas {
        let neg_beta = -beta;
        for &u in &test_u_values {
            let fast = fast_pow_neg_beta(u, neg_beta);
            let reference = reference_pow(u, neg_beta);
            let rel_error = (fast - reference).abs() / reference.abs().max(1e-30);
            assert!(
                rel_error < 1e-6,
                "beta={}, u={}: fast={:.10e}, ref={:.10e}, rel_error={:.2e}",
                beta,
                u,
                fast,
                reference,
                rel_error
            );
        }
    }
}

#[test]
fn test_arbitrary_beta_matches_powf() {
    let test_u_values = [1.1, 1.5, 2.0, 5.0, 10.0, 50.0];
    let arbitrary_betas = [1.3, 2.7, 3.15, 4.1];

    for &beta in &arbitrary_betas {
        let neg_beta = -beta;
        for &u in &test_u_values {
            let fast = fast_pow_neg_beta(u, neg_beta);
            let reference = reference_pow(u, neg_beta);
            let rel_error = (fast - reference).abs() / reference.abs().max(1e-30);
            // exp·ln fallback has slightly lower precision than powf (~1e-6 typical)
            assert!(
                rel_error < 1e-5,
                "beta={}, u={}: fast={:.10e}, ref={:.10e}, rel_error={:.2e}",
                beta,
                u,
                fast,
                reference,
                rel_error
            );
        }
    }
}

#[test]
fn test_u_near_one() {
    // u is always >= 1.0 in Moffat profile (u = 1 + r²/α²)
    let u = 1.0001;
    let neg_beta = -2.5;
    let fast = fast_pow_neg_beta(u, neg_beta);
    let reference = reference_pow(u, neg_beta);
    let rel_error = (fast - reference).abs() / reference.abs();
    assert!(
        rel_error < 1e-6,
        "near 1: fast={:.10e}, ref={:.10e}, rel_error={:.2e}",
        fast,
        reference,
        rel_error
    );
}

#[test]
fn test_large_u() {
    // Large u values (pixel far from center)
    let u = 1000.0;
    for &beta in &[1.0, 2.5, 4.5] {
        let neg_beta = -beta;
        let fast = fast_pow_neg_beta(u, neg_beta);
        let reference = reference_pow(u, neg_beta);
        let rel_error = (fast - reference).abs() / reference.abs().max(1e-30);
        assert!(
            rel_error < 1e-5,
            "large u: beta={}, fast={:.10e}, ref={:.10e}, rel_error={:.2e}",
            beta,
            fast,
            reference,
            rel_error
        );
    }
}

#[test]
fn test_result_is_positive() {
    // u^(negative) should always be positive for positive u
    for &beta in &[1.0, 1.5, 2.0, 2.5, 3.0, 3.5, 4.0, 4.5, 2.7] {
        for &u in &[1.001, 2.0, 10.0, 100.0] {
            let result = fast_pow_neg_beta(u, -beta);
            assert!(
                result > 0.0,
                "beta={}, u={}: result={} should be positive",
                beta,
                u,
                result
            );
        }
    }
}

#[test]
fn test_monotonically_decreasing_in_u() {
    // For fixed negative exponent, u^(neg_beta) should decrease as u increases
    for &beta in &[1.0, 2.5, 4.5, 3.15] {
        let neg_beta = -beta;
        let mut prev = fast_pow_neg_beta(1.0, neg_beta);
        for i in 1..=100 {
            let u = 1.0 + i as f32 * 0.5;
            let curr = fast_pow_neg_beta(u, neg_beta);
            assert!(
                curr < prev,
                "beta={}: not monotonically decreasing at u={}: {} >= {}",
                beta,
                u,
                curr,
                prev
            );
            prev = curr;
        }
    }
}

#[test]
fn test_monotonically_decreasing_in_beta() {
    // For fixed u > 1, increasing beta (more negative neg_beta) should decrease the result
    let u = 3.0;
    let betas = [1.0, 1.5, 2.0, 2.5, 3.0, 3.5, 4.0, 4.5];
    let mut prev = fast_pow_neg_beta(u, -betas[0]);
    for &beta in &betas[1..] {
        let curr = fast_pow_neg_beta(u, -beta);
        assert!(
            curr < prev,
            "u={}: not decreasing at beta={}: {} >= {}",
            u,
            beta,
            curr,
            prev
        );
        prev = curr;
    }
}
