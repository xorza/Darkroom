//! Tests for Moffat profile fitting.

use crate::internals::prelude::*;

use std::f64::consts::PI;

use crate::internals::synthetic::star_profiles::{StarProfile, SyntheticStar};
use crate::star_detection::centroid::lm_optimizer::internals::{
    ModelJacobian, ModelSample, ModelStamp, assert_batch_matches_reference,
    assert_jacobian_matches_differences,
};
use crate::star_detection::centroid::moffat_fit::*;
use crate::star_detection::centroid::tests::perturbation::Perturbation;

/// One Moffat recovery case, as `gaussian_fit`'s `RecoveryCase` with one more axis: `fixed_beta`,
/// the shape the fitter is told to assume, which `wrong_beta` sets apart from the rendered `beta`.
///
/// A clean stamp with the right β is samples of the fitted model itself, held to [`EXACT`] times
/// `(A + B)/A` as there. A noisy one is held to five times the position's Cramér–Rao bound,
/// computed from the model's own Jacobian ([`MoffatCase::position_bound`]). A wrong β has no
/// closed-form bias: it is held to what was measured, 9.2e-3 px, with room.
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
}

/// The relative error a clean stamp's fitted parameters keep where the star dominates its
/// samples; see `gaussian_fit`'s `RecoveryCase`.
const EXACT: f64 = 1e-6;

/// What a wrong β leaves in the centre; see [`MoffatCase`].
const WRONG_BETA_POSITION: f64 = 0.02;

impl MoffatCase {
    /// Five standard deviations of each centre coordinate under the stamp's noise: the Fisher
    /// information on x₀ is `Σ(∂f/∂x₀)²/σₙ²` over the fitted stamp, so `σ_x = σₙ/√Σ(∂f/∂x₀)²`.
    fn position_bound(&self) -> DVec2 {
        let model = MoffatFixedBeta::new(self.fit_radius as f64, f64::from(self.beta), 1e-6);
        let params = [
            self.center.x,
            self.center.y,
            f64::from(self.amplitude),
            f64::from(self.alpha),
            f64::from(self.background),
        ];
        let (cx, cy) = (self.guess.x.round() as isize, self.guess.y.round() as isize);
        let r = self.fit_radius as isize;
        let mut information = DVec2::ZERO;
        for y in cy - r..=cy + r {
            for x in cx - r..=cx + r {
                let row = model
                    .evaluate_and_jacobian(x as f64, y as f64, &params)
                    .jacobian;
                information += DVec2::new(row[0] * row[0], row[1] * row[1]);
            }
        }
        5.0 * f64::from(self.perturbation.rms())
            / DVec2::new(information.x.sqrt(), information.y.sqrt())
    }
}

/// A clean case at the stamp centre with everything but the named fields at the defaults: amplitude
/// 1, α 2.5, β 2.5, sky 0.1, a 21-pixel stamp fitted at radius 8.
const fn clean(name: &'static str) -> MoffatCase {
    MoffatCase {
        name,
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
    }
}

const fn with_beta(name: &'static str, beta: f32) -> MoffatCase {
    MoffatCase {
        beta,
        fixed_beta: beta,
        ..clean(name)
    }
}

const fn with_alpha(name: &'static str, alpha: f32) -> MoffatCase {
    MoffatCase {
        alpha,
        ..clean(name)
    }
}

const MOFFAT_CASES: &[MoffatCase] = &[
    clean("centered"),
    MoffatCase {
        center: DVec2::new(10.3, 10.7),
        ..clean("subpixel_offset")
    },
    MoffatCase {
        guess: DVec2::new(8.0, 12.0),
        ..clean("guess_two_pixels_off")
    },
    MoffatCase {
        fit_background: Some(0.12),
        ..clean("wrong_background_estimate")
    },
    MoffatCase {
        amplitude: 10000.0,
        background: 100.0,
        ..clean("very_high_amplitude")
    },
    MoffatCase {
        amplitude: 0.01,
        background: 0.001,
        ..clean("very_low_amplitude")
    },
    MoffatCase {
        amplitude: 0.1,
        background: 0.5,
        ..clean("faint_on_bright_sky")
    },
    with_alpha("alpha_0.8", 0.8),
    with_alpha("alpha_2", 2.0),
    with_alpha("alpha_3", 3.0),
    with_alpha("alpha_3.5", 3.5),
    MoffatCase {
        stamp: 31,
        center: DVec2::new(15.0, 15.0),
        guess: DVec2::splat(15.0),
        fit_radius: 12,
        ..with_alpha("alpha_6", 6.0)
    },
    // From Lorentzian-like 1.5 to Gaussian-like 6, through every `PowStrategy`.
    with_beta("beta_1.5", 1.5),
    with_beta("beta_2", 2.0),
    with_beta("beta_3", 3.0),
    with_beta("beta_4", 4.0),
    with_beta("beta_4.3", 4.3),
    with_beta("beta_5", 5.0),
    with_beta("beta_6", 6.0),
    MoffatCase {
        perturbation: Perturbation::Gaussian {
            sigma: 0.05,
            seed: 12345,
        },
        ..clean("gaussian_noise")
    },
    MoffatCase {
        perturbation: Perturbation::Gaussian {
            sigma: 0.15,
            seed: 54321,
        },
        ..clean("high_noise")
    },
    MoffatCase {
        center: DVec2::new(10.3, 10.7),
        beta: 4.0,
        ..clean("wrong_beta")
    },
];

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

        let beta = case.fixed_beta;
        let result = MoffatFit::new(
            &pixels,
            case.guess,
            &StampGrid::new(case.fit_radius),
            case.fit_background.unwrap_or(case.background),
            None,
            beta,
        )
        .unwrap_or_else(|| panic!("{}: fit returned None", case.name));

        let offset = (result.pos - case.center).abs();
        if case.fixed_beta != case.beta {
            assert!(
                offset.length() <= WRONG_BETA_POSITION,
                "{}: position off by {}",
                case.name,
                offset.length()
            );
        } else if matches!(case.perturbation, Perturbation::None) {
            let bound =
                EXACT * f64::from(case.amplitude + case.background) / f64::from(case.amplitude);
            let relative = |got: f32, want: f32| f64::from((got - want).abs() / want);
            for (what, error) in [
                ("position", offset.length()),
                ("α", relative(result.debug.alpha, case.alpha)),
                (
                    "amplitude",
                    relative(result.debug.amplitude, case.amplitude),
                ),
                (
                    "background",
                    f64::from((result.debug.background - case.background).abs())
                        / f64::from(case.amplitude),
                ),
                (
                    "FWHM",
                    relative(result.fwhm, alpha_beta_to_fwhm(case.alpha, case.beta)),
                ),
            ] {
                assert!(error <= bound, "{}: {what} off by {error:e}", case.name);
            }
        } else {
            let bound = case.position_bound();
            assert!(
                offset.x <= bound.x && offset.y <= bound.y,
                "{}: position off by {offset} against {bound}",
                case.name
            );
        }
    }
}

/// A stamp with no star, or one that leaves the frame, gives no fit.
#[test]
fn moffat_fit_rejects_what_the_data_cannot_support() {
    let star = SyntheticStar::new(
        Vec2::splat(10.0),
        1.0,
        StarProfile::Moffat {
            alpha: 2.5,
            beta: 2.5,
        },
    )
    .stamp(Size2us::new(21, 21), 0.1);
    let uniform = Buffer2::new_filled(21, 21, 0.5f32);
    for (name, pixels, seed, sky, lands) in [
        ("the star", &star, DVec2::splat(10.0), 0.1, true),
        ("uniform stamp", &uniform, DVec2::splat(10.0), 0.5, false),
        (
            "stamp off the left edge",
            &star,
            DVec2::new(2.0, 10.0),
            0.1,
            false,
        ),
    ] {
        let fit = MoffatFit::new(pixels, seed, &StampGrid::new(8), sky, None, 2.5);
        assert_eq!(fit.is_some(), lands, "{name}");
    }
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

    // Every `PowStrategy`: integers, half-integers and the general power.
    for beta in [2.0, 2.3, 2.5, 3.0, 3.5, 4.5] {
        let model = MoffatFixedBeta::new(15.0, beta, 1e-6);
        for params in params_list {
            for &(x, y) in &points {
                let eval = model.evaluate(x, y, params);
                let jac = model.jacobian_row(x, y, params);
                let ModelSample {
                    value: fused_eval,
                    jacobian: fused_jac,
                } = model.evaluate_and_jacobian(x, y, params);

                // The two share `fast_pow_neg` and differ in operation order: a few ulps.
                assert!(
                    (eval - fused_eval).abs() <= 16.0 * f64::EPSILON * eval.abs().max(1.0),
                    "evaluate mismatch: beta={beta}, eval={eval}, fused={fused_eval}"
                );
                for i in 0..5 {
                    assert!(
                        (jac[i] - fused_jac[i]).abs()
                            <= 16.0 * f64::EPSILON * jac[i].abs().max(1.0),
                        "jacobian[{i}] mismatch: beta={beta}, jac={}, fused={}",
                        jac[i],
                        fused_jac[i]
                    );
                }
            }
            assert_jacobian_matches_differences(&model, params, &points);
        }
    }
}

/// The batch normal equations against the scalar reference at every stamp size through 17 and
/// every `PowStrategy`.
#[test]
fn batch_normal_equations_match_reference() {
    let truth = [6.5, 6.5, 800.0, 2.0, 80.0];
    // Away from the truth, so the residuals are not zero.
    let params = [6.7, 6.3, 790.0, 2.1, 82.0];
    for beta in [2.0, 2.3, 2.5, 3.0, 3.5] {
        let model = MoffatFixedBeta::new(8.0, beta, 1e-6);
        for size in [3, 4, 5, 7, 9, 11, 13, 15, 17] {
            let stamp = ModelStamp::of(&model, size, &truth);
            assert_batch_matches_reference(&model, &stamp, &params);
        }
    }
}
