use std::f64::consts::E;

use crate::registration::ransac::welsch::*;

/// Two closed forms that each round once or twice in the last place of values up to 1.
const TOL: f64 = 4.0 * f64::EPSILON;

#[test]
fn gamma_k2_is_one_minus_exp() {
    for (x, expected) in [
        (-100.0, 0.0),
        (-1.0, 0.0),
        (0.0, 0.0),
        (0.5, 1.0 - (-0.5f64).exp()),
        (1.0, 1.0 - 1.0 / E),
        (2.0, 1.0 - (-2.0f64).exp()),
        (100.0, 1.0),
    ] {
        assert!(
            (gamma_k2(x) - expected).abs() <= TOL,
            "γ(1, {x}) = {}",
            gamma_k2(x)
        );
    }
}

/// `threshold_sq = χ²₀.₉₉(2)·σ²` and `outlier_loss = σ²/2`: exact for σ a power of two.
#[test]
fn scorer_construction_exact() {
    for (sigma, sigma_sq) in [(2.0, 4.0), (0.5, 0.25), (1.0, 1.0)] {
        let scorer = WelschScorer::new(sigma);
        assert_eq!(scorer.threshold_sq, CHI2_99_2DOF * sigma_sq);
        assert_eq!(scorer.outlier_loss, sigma_sq / 2.0);
        assert_eq!(scorer.max_sigma_sq, sigma_sq);
    }
}

/// `loss(r²) = σ²/2 · γ(1, r²/(2σ²))` inside the threshold and `σ²/2` past it. At the threshold
/// itself the quantile makes `exp(−χ²/2)` exactly 0.01, so the loss is `0.99·σ²/2` there and jumps
/// by `0.01·σ²/2` to the outlier loss — the gap a saturating loss leaves.
#[test]
fn scorer_loss_hand_values() {
    let one = WelschScorer::new(1.0);
    for (residual_sq, expected) in [
        (0.0, 0.0),
        (1.0, 0.5 * (1.0 - (-0.5f64).exp())),
        (2.0, 0.5 * (1.0 - 1.0 / E)),
        (4.0, 0.5 * (1.0 - (-2.0f64).exp())),
        (
            CHI2_99_2DOF - 0.01,
            0.5 * (1.0 - (-(CHI2_99_2DOF - 0.01) / 2.0).exp()),
        ),
        (CHI2_99_2DOF, 0.495),
        (CHI2_99_2DOF.next_up(), 0.5),
        (100.0, 0.5),
    ] {
        let loss = one.loss(residual_sq);
        assert!(
            (loss - expected).abs() <= TOL,
            "loss({residual_sq}) = {loss}, expected {expected}"
        );
    }
    // σ = 2 at the same r² = 2: `x = 2/8`, so `2·(1 − e^{−¼})` = 0.4424 against σ = 1's 0.3161.
    let two = WelschScorer::new(2.0);
    assert!((two.loss(2.0) - 2.0 * (1.0 - (-0.25f64).exp())).abs() <= TOL);
}

/// The loss never falls as the residual grows.
#[test]
fn scorer_loss_is_monotone() {
    let scorer = WelschScorer::new(1.5);
    let mut previous = 0.0;
    for step in 0..=400 {
        let loss = scorer.loss(f64::from(step) * 0.1);
        assert!(
            loss >= previous,
            "loss fell at r² = {}",
            f64::from(step) * 0.1
        );
        previous = loss;
    }
}

/// The inlier test is `r² ≤ χ²·σ²`, inclusive.
#[test]
fn is_inlier_exact_threshold() {
    let scorer = WelschScorer::new(1.0);
    assert!(scorer.is_inlier(0.0));
    assert!(scorer.is_inlier(CHI2_99_2DOF));
    assert!(!scorer.is_inlier(CHI2_99_2DOF.next_up()));
}

/// The boundary in pixels is `√χ²₀.₉₉(2)·σ` = 3.034 854 3·σ — the figure `tuning::recovery_radius`
/// has to reproduce for the two gates to agree.
#[test]
fn effective_threshold_exact() {
    let sqrt_chi = CHI2_99_2DOF.sqrt();
    assert!((sqrt_chi - 3.034_854_3).abs() < 1e-7, "√χ² = {sqrt_chi}");
    for sigma in [1.0, 3.0] {
        let threshold = WelschScorer::new(sigma).threshold_sq.sqrt();
        assert!(
            (threshold - sqrt_chi * sigma).abs() <= TOL * sigma * 4.0,
            "σ = {sigma}"
        );
    }
}
