//! Tests for Moffat profile fitting.
use crate::stacking::star_detection::centroid::StampGrid;
use crate::testing::prelude::*;

use std::f64::consts::PI;

use crate::stacking::star_detection::centroid::internals::{
    Perturbation, reference_normal_equations,
};
use crate::stacking::star_detection::centroid::lm_optimizer::LMConfig;
use crate::stacking::star_detection::centroid::moffat_fit::*;
use crate::testing::synthetic::star_profiles::{StarProfile, SyntheticStar};

/// One Moffat recovery case, mirroring `gaussian_fit`'s `RecoveryCase`.
///
/// The extra axis over the Gaussian version is `fixed_beta`: the fitter is told what shape to
/// assume, and `wrong_beta` deliberately tells it the wrong one. Tolerances stay per-case for
/// the same reason as there — they span 0.05 to 0.5 and a shared bound would loosen the strict
/// ones.
#[derive(Debug)]
struct MoffatCase {
    name: &'static str,
    stamp: usize,
    center: DVec2,
    amplitude: f32,
    alpha: f32,
    /// Shape the stamp is rendered with.
    beta: f32,
    /// Shape the fitter is told to assume; differs from `beta` only in `wrong_beta`.
    fixed_beta: f32,
    background: f32,
    guess: DVec2,
    fit_radius: usize,
    perturbation: Perturbation,
    /// Background handed to the fitter; `None` gives it the true one.
    fit_background: Option<f32>,
    pos_tol: Option<f64>,
    alpha_tol: Option<f32>,
    background_tol: Option<f32>,
}

const MOFFAT_CASES: &[MoffatCase] = &[
    MoffatCase {
        name: "centered_fixed_beta",
        stamp: 21,
        center: DVec2::new(10.0, 10.0),
        amplitude: 1.0,
        alpha: 2.5,
        beta: 2.5,
        fixed_beta: 2.5,
        background: 0.1,
        guess: DVec2::splat(10.0),
        fit_radius: 8,
        perturbation: Perturbation::None,
        fit_background: None,
        pos_tol: Some(0.1),
        alpha_tol: Some(0.3),
        background_tol: None,
    },
    MoffatCase {
        name: "subpixel_offset",
        stamp: 21,
        center: DVec2::new(10.3, 10.7),
        amplitude: 1.0,
        alpha: 2.5,
        beta: 2.5,
        fixed_beta: 2.5,
        background: 0.1,
        guess: DVec2::splat(10.0),
        fit_radius: 8,
        perturbation: Perturbation::None,
        fit_background: None,
        pos_tol: Some(0.05),
        alpha_tol: None,
        background_tol: None,
    },
    MoffatCase {
        // Noise sigma is 5% of amplitude.
        name: "gaussian_noise",
        stamp: 21,
        center: DVec2::new(10.0, 10.0),
        amplitude: 1.0,
        alpha: 2.5,
        beta: 2.5,
        fixed_beta: 2.5,
        background: 0.1,
        guess: DVec2::splat(10.0),
        fit_radius: 8,
        perturbation: Perturbation::Gaussian {
            sigma: 0.05,
            seed: 12345,
        },
        fit_background: None,
        pos_tol: Some(0.2),
        alpha_tol: None,
        background_tol: None,
    },
    MoffatCase {
        name: "high_noise",
        stamp: 21,
        center: DVec2::new(10.0, 10.0),
        amplitude: 1.0,
        alpha: 2.5,
        beta: 2.5,
        fixed_beta: 2.5,
        background: 0.1,
        guess: DVec2::splat(10.0),
        fit_radius: 8,
        perturbation: Perturbation::Gaussian {
            sigma: 0.15,
            seed: 54321,
        },
        fit_background: None,
        pos_tol: Some(0.5),
        alpha_tol: None,
        background_tol: None,
    },
    MoffatCase {
        // Fitter is handed a background 20% too high and must recover the true one.
        name: "wrong_background_estimate",
        stamp: 21,
        center: DVec2::new(10.0, 10.0),
        amplitude: 1.0,
        alpha: 2.5,
        beta: 2.5,
        fixed_beta: 2.5,
        background: 0.1,
        guess: DVec2::splat(10.0),
        fit_radius: 8,
        perturbation: Perturbation::None,
        fit_background: Some(0.12),
        pos_tol: Some(0.1),
        alpha_tol: None,
        background_tol: Some(0.05),
    },
    MoffatCase {
        // Rendered at beta 4.0 but fitted assuming 2.5: the centroid must survive the wrong
        // shape, so no claim is made about the recovered alpha.
        name: "wrong_beta",
        stamp: 21,
        center: DVec2::new(10.3, 10.7),
        amplitude: 1.0,
        alpha: 2.5,
        beta: 4.0,
        fixed_beta: 2.5,
        background: 0.1,
        guess: DVec2::splat(10.0),
        fit_radius: 8,
        perturbation: Perturbation::None,
        fit_background: None,
        pos_tol: Some(0.15),
        alpha_tol: None,
        background_tol: None,
    },
    MoffatCase {
        name: "very_high_amplitude",
        stamp: 21,
        center: DVec2::new(10.0, 10.0),
        amplitude: 10000.0,
        alpha: 2.5,
        beta: 2.5,
        fixed_beta: 2.5,
        background: 100.0,
        guess: DVec2::splat(10.0),
        fit_radius: 8,
        perturbation: Perturbation::None,
        fit_background: None,
        pos_tol: Some(0.1),
        alpha_tol: None,
        background_tol: None,
    },
    MoffatCase {
        name: "very_low_amplitude",
        stamp: 21,
        center: DVec2::new(10.0, 10.0),
        amplitude: 0.01,
        alpha: 2.5,
        beta: 2.5,
        fixed_beta: 2.5,
        background: 0.001,
        guess: DVec2::splat(10.0),
        fit_radius: 8,
        perturbation: Perturbation::None,
        fit_background: None,
        pos_tol: Some(0.1),
        alpha_tol: None,
        background_tol: None,
    },
    MoffatCase {
        name: "narrow_psf",
        stamp: 21,
        center: DVec2::new(10.0, 10.0),
        amplitude: 1.0,
        alpha: 0.8,
        beta: 2.5,
        fixed_beta: 2.5,
        background: 0.1,
        guess: DVec2::splat(10.0),
        fit_radius: 8,
        perturbation: Perturbation::None,
        fit_background: None,
        pos_tol: Some(0.1),
        alpha_tol: None,
        background_tol: None,
    },
    MoffatCase {
        name: "wide_psf",
        stamp: 31,
        center: DVec2::new(15.0, 15.0),
        amplitude: 1.0,
        alpha: 6.0,
        beta: 2.5,
        fixed_beta: 2.5,
        background: 0.1,
        guess: DVec2::splat(15.0),
        fit_radius: 12,
        perturbation: Perturbation::None,
        fit_background: None,
        pos_tol: Some(0.1),
        alpha_tol: Some(0.5),
        background_tol: None,
    },
];

/// SIMD and scalar accumulate the same ~225 f64 terms in a different order, so they differ by
/// FMA and reassociation rounding — a few ulp relative, never a structural disagreement.
const SIMD_TOL: f64 = 1e-10;

#[test]
fn moffat_fit_recovers_known_parameters() {
    for case in MOFFAT_CASES {
        let mut pixels = SyntheticStar::new(
            case.center.as_vec2(),
            case.amplitude,
            StarProfile::Moffat {
                alpha: case.alpha,
                beta: case.beta,
            },
        )
        .stamp(Size2us::new(case.stamp, case.stamp), case.background);
        case.perturbation.apply(&mut pixels);

        let config = MoffatFitConfig {
            fixed_beta: case.fixed_beta,
            ..Default::default()
        };
        let result = MoffatFit::new(
            &pixels,
            case.guess,
            &StampGrid::new(case.fit_radius),
            case.fit_background.unwrap_or(case.background),
            None,
            &config,
        )
        .unwrap_or_else(|| panic!("{}: fit returned None", case.name));

        // Every case converges; none of the originals tolerated a non-converged fit.
        assert!(result.converged, "{}: did not converge", case.name);
        assert!(
            result.pos.x.is_finite() && result.pos.y.is_finite(),
            "{}: non-finite position {:?}",
            case.name,
            result.pos
        );

        if let Some(tol) = case.pos_tol {
            assert!(
                (result.pos.x - case.center.x).abs() < tol,
                "{}: x {} vs {} (tol {tol})",
                case.name,
                result.pos.x,
                case.center.x
            );
            assert!(
                (result.pos.y - case.center.y).abs() < tol,
                "{}: y {} vs {} (tol {tol})",
                case.name,
                result.pos.y,
                case.center.y
            );
        }
        if let Some(tol) = case.alpha_tol {
            assert!(
                (result.debug.alpha - case.alpha).abs() < tol,
                "{}: alpha {} vs {} (tol {tol})",
                case.name,
                result.debug.alpha,
                case.alpha
            );
        }
        if let Some(tol) = case.background_tol {
            assert!(
                (result.debug.background - case.background).abs() < tol,
                "{}: background {} vs {} (tol {tol})",
                case.name,
                result.debug.background,
                case.background
            );
        }
    }
}

#[test]
fn alpha_beta_fwhm_conversion() {
    let alpha = 2.0;
    let beta = 2.5;
    let fwhm = alpha_beta_to_fwhm(alpha, beta);
    let alpha_back = fwhm_beta_to_alpha(fwhm, beta);
    assert!((alpha_back - alpha).abs() < 1e-6);
    assert!((fwhm - 2.26).abs() < 0.1);
}

#[test]
fn moffat_fit_edge_position() {
    let width = 21;
    let height = 21;
    let pixels = Buffer2::new_filled(width, height, 0.1f32);

    let config = MoffatFitConfig::default();
    let result = MoffatFit::new(
        &pixels,
        DVec2::new(2.0, 10.0),
        &StampGrid::new(8),
        0.1,
        None,
        &config,
    );
    assert!(result.is_none());
}

#[test]
fn moffat_fit_low_snr() {
    // Low SNR (amp=0.1, bg=0.5, SNR~0.2) - L-M should still converge
    // but with reduced accuracy compared to high-SNR case
    let width = 21;
    let height = 21;
    let true_cx = 10.0;
    let true_cy = 10.0;
    let true_amp = 0.1;
    let true_alpha = 2.5;
    let true_beta = 2.5;
    let true_bg = 0.5;

    let pixels = SyntheticStar::new(
        Vec2::new(true_cx as f32, true_cy as f32),
        true_amp,
        StarProfile::Moffat {
            alpha: true_alpha,
            beta: true_beta,
        },
    )
    .stamp(Size2us::new(width, height), true_bg);

    let config = MoffatFitConfig {
        fixed_beta: true_beta,
        ..Default::default()
    };
    let result = MoffatFit::new(
        &pixels,
        DVec2::splat(10.0),
        &StampGrid::new(8),
        true_bg,
        None,
        &config,
    );

    // Even at low SNR, the noiseless Moffat should still be recoverable
    let result = result.expect("Low-SNR Moffat should converge");
    let pos_error = ((result.pos.x - true_cx).powi(2) + (result.pos.y - true_cy).powi(2)).sqrt();
    assert!(
        pos_error < 0.5,
        "Low-SNR position error {:.3} should be < 0.5 px",
        pos_error
    );
    assert!(
        (result.debug.alpha - true_alpha).abs() < 1.0,
        "Low-SNR alpha error {:.3} too large",
        (result.debug.alpha - true_alpha).abs()
    );
}

#[test]
fn moffat_fit_various_beta_values() {
    let width = 21;
    let height = 21;
    let true_cx = 10.0;
    let true_cy = 10.0;
    let true_amp = 1.0;
    let true_alpha = 2.5;
    let true_bg = 0.1;

    // Test various beta values from Lorentzian-like (1.5) to Gaussian-like (6.0)
    for &true_beta in &[1.5f32, 2.0, 2.5, 3.0, 4.0, 5.0, 6.0] {
        let pixels = SyntheticStar::new(
            Vec2::new(true_cx as f32, true_cy as f32),
            true_amp,
            StarProfile::Moffat {
                alpha: true_alpha,
                beta: true_beta,
            },
        )
        .stamp(Size2us::new(width, height), true_bg);

        let config = MoffatFitConfig {
            fixed_beta: true_beta,
            ..Default::default()
        };
        let result = MoffatFit::new(
            &pixels,
            DVec2::splat(10.0),
            &StampGrid::new(8),
            true_bg,
            None,
            &config,
        );

        assert!(result.is_some(), "Failed for beta={}", true_beta);
        let result = result.unwrap();
        assert!(
            result.converged,
            "Failed to converge for beta={}",
            true_beta
        );
        assert!(
            (result.pos.x - true_cx).abs() < 0.1,
            "beta={}: x error={}",
            true_beta,
            (result.pos.x - true_cx).abs()
        );
    }
}

#[test]
fn moffat_fit_converges_within_max_iterations() {
    let width = 21;
    let height = 21;
    let true_cx = 10.0;
    let true_cy = 10.0;
    let true_amp = 1.0;
    let true_alpha = 2.5;
    let true_beta = 2.5;
    let true_bg = 0.1;

    let pixels = SyntheticStar::new(
        Vec2::new(true_cx as f32, true_cy as f32),
        true_amp,
        StarProfile::Moffat {
            alpha: true_alpha,
            beta: true_beta,
        },
    )
    .stamp(Size2us::new(width, height), true_bg);

    let config = MoffatFitConfig {
        fixed_beta: true_beta,
        lm: LMConfig {
            max_iterations: 20, // Moderate iteration limit
            ..Default::default()
        },
    };
    let result = MoffatFit::new(
        &pixels,
        DVec2::splat(10.0),
        &StampGrid::new(8),
        true_bg,
        None,
        &config,
    );

    assert!(result.is_some());
    let result = result.unwrap();
    // Should converge quickly for perfect data
    assert!(result.converged);
    assert!(result.debug.iterations <= 20);
}

#[test]
fn moffat_fit_bad_initial_guess_still_converges() {
    let width = 21;
    let height = 21;
    let true_cx = 10.0;
    let true_cy = 10.0;
    let true_amp = 1.0;
    let true_alpha = 2.5;
    let true_beta = 2.5;
    let true_bg = 0.1;

    let pixels = SyntheticStar::new(
        Vec2::new(true_cx as f32, true_cy as f32),
        true_amp,
        StarProfile::Moffat {
            alpha: true_alpha,
            beta: true_beta,
        },
    )
    .stamp(Size2us::new(width, height), true_bg);

    let config = MoffatFitConfig {
        fixed_beta: true_beta,
        lm: LMConfig {
            max_iterations: 100,
            ..Default::default()
        },
    };

    // Start from a position offset by 2 pixels
    let result = MoffatFit::new(
        &pixels,
        DVec2::new(8.0, 12.0),
        &StampGrid::new(8),
        true_bg,
        None,
        &config,
    );

    assert!(result.is_some());
    let result = result.unwrap();
    assert!(result.converged);
    assert!(
        (result.pos.x - true_cx).abs() < 0.1,
        "x error: {}",
        (result.pos.x - true_cx).abs()
    );
    assert!(
        (result.pos.y - true_cy).abs() < 0.1,
        "y error: {}",
        (result.pos.y - true_cy).abs()
    );
}

#[test]
fn moffat_fit_uniform_data_returns_result() {
    // Uniform data (no star) - should still return a result, though meaningless
    let width = 21;
    let height = 21;
    let uniform_value = 0.5f32;
    let pixels = Buffer2::new_filled(width, height, uniform_value);

    let config = MoffatFitConfig::default();
    let result = MoffatFit::new(
        &pixels,
        DVec2::splat(10.0),
        &StampGrid::new(8),
        uniform_value,
        None,
        &config,
    );

    // Should produce some result (may not converge well)
    assert!(result.is_some());
    let result = result.unwrap();
    // Values should be finite
    assert!(result.pos.x.is_finite());
    assert!(result.pos.y.is_finite());
    assert!(result.debug.amplitude.is_finite());
}

#[test]
fn moffat_fwhm_computed_correctly() {
    let width = 21;
    let height = 21;
    let true_cx = 10.0;
    let true_cy = 10.0;
    let true_amp = 1.0;
    let true_alpha = 2.5;
    let true_beta = 2.5;
    let true_bg = 0.1;

    let pixels = SyntheticStar::new(
        Vec2::new(true_cx as f32, true_cy as f32),
        true_amp,
        StarProfile::Moffat {
            alpha: true_alpha,
            beta: true_beta,
        },
    )
    .stamp(Size2us::new(width, height), true_bg);

    let config = MoffatFitConfig {
        fixed_beta: true_beta,
        ..Default::default()
    };
    let result = MoffatFit::new(
        &pixels,
        DVec2::splat(10.0),
        &StampGrid::new(8),
        true_bg,
        None,
        &config,
    )
    .unwrap();

    // FWHM should match analytical formula
    let expected_fwhm = alpha_beta_to_fwhm(true_alpha, true_beta);
    assert!(
        (result.fwhm - expected_fwhm).abs() < 0.2,
        "FWHM error: {} vs expected {}",
        result.fwhm,
        expected_fwhm
    );
}

#[test]
fn fwhm_increases_with_alpha() {
    let beta = 2.5;
    let fwhm1 = alpha_beta_to_fwhm(1.0, beta);
    let fwhm2 = alpha_beta_to_fwhm(2.0, beta);
    let fwhm3 = alpha_beta_to_fwhm(3.0, beta);

    assert!(fwhm2 > fwhm1);
    assert!(fwhm3 > fwhm2);
    // Should be linear with alpha
    assert!((fwhm2 / fwhm1 - 2.0).abs() < 0.01);
    assert!((fwhm3 / fwhm1 - 3.0).abs() < 0.01);
}

#[test]
fn fwhm_decreases_with_beta() {
    let alpha = 2.0;
    let fwhm_low_beta = alpha_beta_to_fwhm(alpha, 1.5);
    let fwhm_mid_beta = alpha_beta_to_fwhm(alpha, 2.5);
    let fwhm_high_beta = alpha_beta_to_fwhm(alpha, 5.0);

    // Higher beta = narrower profile = smaller FWHM
    assert!(fwhm_high_beta < fwhm_mid_beta);
    assert!(fwhm_mid_beta < fwhm_low_beta);
}

#[test]
fn select_pow_strategy_integers() {
    for beta in [1.0, 2.0, 3.0, 4.0, 5.0] {
        let strategy = select_pow_strategy(beta);
        assert!(
            matches!(strategy, PowStrategy::Int { .. }),
            "beta={beta} should select Int strategy, got {strategy:?}"
        );
    }
}

#[test]
fn select_pow_strategy_half_integers() {
    for beta in [1.5, 2.5, 3.5, 4.5, 5.5] {
        let strategy = select_pow_strategy(beta);
        assert!(
            matches!(strategy, PowStrategy::HalfInt { .. }),
            "beta={beta} should select HalfInt strategy, got {strategy:?}"
        );
    }
}

#[test]
fn select_pow_strategy_general() {
    for beta in [2.3, 3.7, 1.1, PI] {
        let strategy = select_pow_strategy(beta);
        assert!(
            matches!(strategy, PowStrategy::General { .. }),
            "beta={beta} should select General strategy, got {strategy:?}"
        );
    }
}

#[test]
fn fast_pow_neg_accuracy_half_integers() {
    let u_values = [1.01, 1.1, 1.5, 2.0, 5.0, 10.0, 100.0];
    let betas = [1.5, 2.5, 3.5, 4.5, 5.5];

    for &beta in &betas {
        let strategy = select_pow_strategy(beta);
        for &u in &u_values {
            let fast = fast_pow_neg(u, strategy);
            let reference = u.powf(-beta);
            let rel_err = ((fast - reference) / reference).abs();
            assert!(
                rel_err < 1e-14,
                "fast_pow_neg(u={u}, beta={beta}) = {fast}, expected {reference}, rel_err={rel_err}"
            );
        }
    }
}

#[test]
fn fast_pow_neg_accuracy_integers() {
    let u_values = [1.01, 1.1, 2.0, 5.0, 10.0];
    let betas = [1.0, 2.0, 3.0, 4.0, 5.0];

    for &beta in &betas {
        let strategy = select_pow_strategy(beta);
        for &u in &u_values {
            let fast = fast_pow_neg(u, strategy);
            let reference = u.powf(-beta);
            let rel_err = ((fast - reference) / reference).abs();
            assert!(
                rel_err < 1e-14,
                "fast_pow_neg(u={u}, beta={beta}) = {fast}, expected {reference}, rel_err={rel_err}"
            );
        }
    }
}

#[test]
fn fast_pow_neg_general_fallback() {
    let beta = 2.3;
    let strategy = select_pow_strategy(beta);
    let u = 3.0;
    let fast = fast_pow_neg(u, strategy);
    let reference = u.powf(-beta);
    assert!(
        (fast - reference).abs() < 1e-15,
        "General fallback should be identical to powf"
    );
}

#[test]
fn int_pow_correctness() {
    let u = 2.5;
    assert!((int_pow(u, 0) - 1.0).abs() < 1e-15);
    assert!((int_pow(u, 1) - u).abs() < 1e-15);
    assert!((int_pow(u, 2) - u * u).abs() < 1e-15);
    assert!((int_pow(u, 3) - u * u * u).abs() < 1e-14);
    assert!((int_pow(u, 4) - u.powi(4)).abs() < 1e-13);
    assert!((int_pow(u, 5) - u.powi(5)).abs() < 1e-12);
    assert!((int_pow(u, 6) - u.powi(6)).abs() < 1e-11);
    assert!((int_pow(u, 10) - u.powi(10)).abs() < 1e-6);
}

#[test]
fn moffat_fixed_beta_evaluate_and_jacobian_consistency() {
    let params_list: &[[f64; 5]] = &[
        [10.0, 10.0, 1000.0, 2.0, 100.0],
        [5.5, 7.3, 500.0, 3.0, 50.0],
        [0.0, 0.0, 1.0, 1.0, 0.0],
    ];
    let points = [(8.0, 9.0), (10.0, 10.0), (12.0, 11.0), (5.0, 7.0)];

    for beta in [2.0, 2.5, 3.0, 3.5, 4.5] {
        let model = MoffatFixedBeta::new(15.0, beta);
        for params in params_list {
            for &(x, y) in &points {
                let eval = model.evaluate(x, y, params);
                let jac = model.jacobian_row(x, y, params);
                let (fused_eval, fused_jac) = model.evaluate_and_jacobian(x, y, params);

                assert!(
                    (eval - fused_eval).abs() < 1e-15,
                    "evaluate mismatch: beta={beta}, eval={eval}, fused={fused_eval}"
                );
                for i in 0..5 {
                    assert!(
                        (jac[i] - fused_jac[i]).abs() < 1e-14,
                        "jacobian[{i}] mismatch: beta={beta}, jac={}, fused={}",
                        jac[i],
                        fused_jac[i]
                    );
                }
            }
        }
    }
}

/// Build stamp data arrays (x, y, z) for a Moffat profile at given params.
fn make_stamp_data(size: usize, params: &[f64; 5], beta: f64) -> (Vec<f64>, Vec<f64>, Vec<f64>) {
    let [x0, y0, amp, alpha, bg] = *params;
    let alpha2 = alpha * alpha;
    let mut data_x = Vec::with_capacity(size * size);
    let mut data_y = Vec::with_capacity(size * size);
    let mut data_z = Vec::with_capacity(size * size);
    for iy in 0..size {
        for ix in 0..size {
            let x = ix as f64;
            let y = iy as f64;
            let r2 = (x - x0).powi(2) + (y - y0).powi(2);
            let z = amp * (1.0 + r2 / alpha2).powf(-beta) + bg;
            data_x.push(x);
            data_y.push(y);
            data_z.push(z);
        }
    }
    (data_x, data_y, data_z)
}

#[test]
fn batch_build_normal_equations_matches_scalar() {
    use crate::stacking::star_detection::centroid::lm_optimizer::LMModel;

    let beta = 2.5;
    let true_params = [6.3, 6.7, 1000.0, 2.5, 100.0];
    // Use offset params so residuals are non-trivial
    let params = [6.5, 6.5, 980.0, 2.6, 102.0];
    let model = MoffatFixedBeta::new(8.0, beta);
    let (data_x, data_y, data_z) = make_stamp_data(13, &true_params, beta);

    // Scalar reference: build jacobian/residuals then compute hessian/gradient
    let mut jac_scalar = Vec::new();
    let mut res_scalar = Vec::new();
    for ((&x, &y), &z) in data_x.iter().zip(data_y.iter()).zip(data_z.iter()) {
        let (model_val, jac_row) = model.evaluate_and_jacobian(x, y, &params);
        jac_scalar.push(jac_row);
        res_scalar.push(z - model_val);
    }
    let NormalEquations {
        hessian: hessian_scalar,
        gradient: gradient_scalar,
        chi2: chi2_scalar,
    } = reference_normal_equations(&jac_scalar, &res_scalar);

    // Batch path (uses SIMD on x86_64 with AVX2)
    let NormalEquations {
        hessian: hessian_batch,
        gradient: gradient_batch,
        chi2: chi2_batch,
    } = model.batch_build_normal_equations(FitData::unweighted(&data_x, &data_y, &data_z), &params);

    // Chi² should match
    assert_close!(
        chi2_scalar,
        chi2_batch,
        SIMD_TOL,
        "chi2 mismatch: scalar={chi2_scalar}, batch={chi2_batch}"
    );

    // Gradient should match
    assert_close_slice!(gradient_scalar, gradient_batch, SIMD_TOL, "gradient");

    // Hessian should match (full matrix including mirrored lower triangle)
    for i in 0..5 {
        for j in 0..5 {
            assert_close!(
                hessian_scalar[i][j],
                hessian_batch[i][j],
                SIMD_TOL,
                "hessian[{i}][{j}] mismatch: scalar={}, batch={}",
                hessian_scalar[i][j],
                hessian_batch[i][j]
            );
        }
    }
}

#[test]
fn batch_compute_chi2_matches_scalar() {
    use crate::stacking::star_detection::centroid::lm_optimizer::LMModel;

    let beta = 2.5;
    let model = MoffatFixedBeta::new(8.0, beta);
    // Use slightly off params so residuals are non-zero
    let true_params = [6.3, 6.7, 1000.0, 2.5, 100.0];
    let test_params = [6.5, 6.5, 980.0, 2.6, 102.0];
    let (data_x, data_y, data_z) = make_stamp_data(13, &true_params, beta);

    // Scalar chi²
    let chi2_scalar: f64 = data_x
        .iter()
        .zip(data_y.iter())
        .zip(data_z.iter())
        .map(|((&x, &y), &z)| {
            let r = z - model.evaluate(x, y, &test_params);
            r * r
        })
        .sum();

    // Batch chi² (uses SIMD on x86_64 with AVX2)
    let chi2_batch =
        model.batch_compute_chi2(FitData::unweighted(&data_x, &data_y, &data_z), &test_params);

    assert_close!(
        chi2_scalar,
        chi2_batch,
        SIMD_TOL,
        "chi2 mismatch: scalar={chi2_scalar}, batch={chi2_batch}, diff={}",
        (chi2_scalar - chi2_batch).abs()
    );
}

/// Weighted data must bypass the SIMD kernels (which are unweighted-only) and still apply the
/// weights, so uniform weights reproduce the unweighted result and a uniform `w` scales it by `w`.
#[test]
fn batch_weighted_bypasses_simd_and_applies_weights() {
    use crate::stacking::star_detection::centroid::lm_optimizer::LMModel;

    let beta = 2.5;
    let true_params = [6.3, 6.7, 1000.0, 2.5, 100.0];
    let params = [6.5, 6.5, 980.0, 2.6, 102.0];
    let model = MoffatFixedBeta::new(8.0, beta);
    let (data_x, data_y, data_z) = make_stamp_data(13, &true_params, beta);

    let unweighted =
        model.batch_build_normal_equations(FitData::unweighted(&data_x, &data_y, &data_z), &params);

    for scale in [1.0f64, 2.0] {
        let weights = vec![scale; data_x.len()];
        let weighted = model.batch_build_normal_equations(
            FitData::new(&data_x, &data_y, &data_z, Some(&weights)),
            &params,
        );

        // Every term of the normal equations is linear in the per-pixel weight.
        assert_close!(
            weighted.chi2,
            scale * unweighted.chi2,
            SIMD_TOL,
            "chi2 at w={scale}: {} != {} * {}",
            weighted.chi2,
            scale,
            unweighted.chi2
        );
        for i in 0..5 {
            assert_close!(
                weighted.gradient[i],
                scale * unweighted.gradient[i],
                SIMD_TOL,
                "gradient[{i}] at w={scale}: {} != {} * {}",
                weighted.gradient[i],
                scale,
                unweighted.gradient[i]
            );
            for j in 0..5 {
                assert_close!(
                    weighted.hessian[i][j],
                    scale * unweighted.hessian[i][j],
                    SIMD_TOL,
                    "hessian[{i}][{j}] at w={scale}: {} != {} * {}",
                    weighted.hessian[i][j],
                    scale,
                    unweighted.hessian[i][j]
                );
            }
        }
    }
}

#[test]
fn batch_build_normal_equations_various_stamp_sizes() {
    use crate::stacking::star_detection::centroid::lm_optimizer::LMModel;

    let beta = 2.5;
    let model = MoffatFixedBeta::new(10.0, beta);
    let true_params = [5.0, 5.0, 500.0, 2.0, 50.0];
    // Offset params for non-trivial residuals
    let params = [5.2, 4.8, 490.0, 2.1, 51.0];

    // Test sizes that exercise: exact multiple of 4, remainder 1, 2, 3
    for size in [3, 4, 5, 7, 9, 11, 13, 15, 17] {
        let (data_x, data_y, data_z) = make_stamp_data(size, &true_params, beta);

        // Scalar reference
        let mut jac_scalar = Vec::new();
        let mut res_scalar = Vec::new();
        for ((&x, &y), &z) in data_x.iter().zip(data_y.iter()).zip(data_z.iter()) {
            let (model_val, jac_row) = model.evaluate_and_jacobian(x, y, &params);
            jac_scalar.push(jac_row);
            res_scalar.push(z - model_val);
        }
        let NormalEquations {
            hessian: hessian_scalar,
            gradient: gradient_scalar,
            chi2: chi2_scalar,
        } = reference_normal_equations(&jac_scalar, &res_scalar);

        // Batch
        let NormalEquations {
            hessian: hessian_batch,
            gradient: gradient_batch,
            chi2: chi2_batch,
        } = model
            .batch_build_normal_equations(FitData::unweighted(&data_x, &data_y, &data_z), &params);

        assert_close!(
            chi2_scalar,
            chi2_batch,
            SIMD_TOL,
            "size={size}: chi2 mismatch: scalar={chi2_scalar}, batch={chi2_batch}"
        );

        for i in 0..5 {
            assert_close!(
                gradient_scalar[i],
                gradient_batch[i],
                SIMD_TOL,
                "size={size}: gradient[{i}] mismatch: scalar={}, batch={}",
                gradient_scalar[i],
                gradient_batch[i]
            );
            for j in 0..5 {
                assert_close!(
                    hessian_scalar[i][j],
                    hessian_batch[i][j],
                    SIMD_TOL,
                    "size={size}: hessian[{i}][{j}] mismatch: scalar={}, batch={}",
                    hessian_scalar[i][j],
                    hessian_batch[i][j]
                );
            }
        }
    }
}

#[test]
fn batch_build_normal_equations_all_pow_strategies() {
    use crate::stacking::star_detection::centroid::lm_optimizer::LMModel;

    let true_params = [6.5, 6.5, 800.0, 2.0, 80.0];
    // Offset params for non-trivial residuals
    let params = [6.7, 6.3, 790.0, 2.1, 82.0];

    // HalfInt: 2.5, 3.5; Int: 2.0, 3.0; General: 2.3
    for beta in [2.0, 2.3, 2.5, 3.0, 3.5] {
        let model = MoffatFixedBeta::new(8.0, beta);
        let (data_x, data_y, data_z) = make_stamp_data(13, &true_params, beta);

        // Scalar reference
        let mut jac_scalar = Vec::new();
        let mut res_scalar = Vec::new();
        for ((&x, &y), &z) in data_x.iter().zip(data_y.iter()).zip(data_z.iter()) {
            let (model_val, jac_row) = model.evaluate_and_jacobian(x, y, &params);
            jac_scalar.push(jac_row);
            res_scalar.push(z - model_val);
        }
        let NormalEquations {
            hessian: hessian_scalar,
            gradient: gradient_scalar,
            chi2: chi2_scalar,
        } = reference_normal_equations(&jac_scalar, &res_scalar);

        // Batch
        let NormalEquations {
            hessian: hessian_batch,
            gradient: gradient_batch,
            chi2: chi2_batch,
        } = model
            .batch_build_normal_equations(FitData::unweighted(&data_x, &data_y, &data_z), &params);

        assert_close!(
            chi2_scalar,
            chi2_batch,
            SIMD_TOL,
            "beta={beta}: chi2 mismatch: scalar={chi2_scalar}, batch={chi2_batch}"
        );

        for i in 0..5 {
            assert_close!(
                gradient_scalar[i],
                gradient_batch[i],
                SIMD_TOL,
                "beta={beta}: gradient[{i}] mismatch: scalar={}, batch={}",
                gradient_scalar[i],
                gradient_batch[i]
            );
            for j in 0..5 {
                assert_close!(
                    hessian_scalar[i][j],
                    hessian_batch[i][j],
                    SIMD_TOL,
                    "beta={beta}: hessian[{i}][{j}] mismatch: scalar={}, batch={}",
                    hessian_scalar[i][j],
                    hessian_batch[i][j]
                );
            }
        }
    }
}

/// Validation must reject a non-finite fit rather than pass it through: every comparison against
/// NaN is false, so a check phrased as rejections ("bail if alpha > max") accepts one.
#[test]
fn validate_position_rejects_non_finite_and_keeps_its_bounds() {
    let at = DVec2::splat(8.0);
    let radius = 8usize;
    // Baseline: a centred, plausibly-sized fit is accepted, so the rejections below are the
    // non-finite values and not some unrelated bound.
    assert!(validate_position(at, at, 2.0, radius));

    for bad in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
        assert!(!validate_position(at, at, bad, radius), "alpha = {bad}");
        let moved = DVec2::new(bad as f64, 8.0);
        assert!(!validate_position(moved, at, 2.0, radius), "pos.x = {bad}");
    }

    // Bounds are inclusive at both ends, and one step outside each is rejected.
    assert!(validate_position(at, at, 0.5, radius));
    assert!(validate_position(at, at, 16.0, radius));
    assert!(!validate_position(at, at, 0.49, radius));
    assert!(!validate_position(at, at, 16.01, radius));
    // Centre exactly `stamp_radius` away is still inside; beyond it is not.
    assert!(validate_position(DVec2::new(16.0, 8.0), at, 2.0, radius));
    assert!(!validate_position(DVec2::new(16.01, 8.0), at, 2.0, radius));
}
