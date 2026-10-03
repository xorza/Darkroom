//! Tests for 2D Gaussian fitting.
use crate::internals::prelude::*;
use crate::internals::synthetic::patterns;
use std::f32::consts::FRAC_PI_4;
use std::f64::consts::PI;

use crate::internals::synthetic::star_profiles::{StarProfile, SyntheticStar};
use crate::star_detection::centroid::gaussian_fit::*;
use crate::star_detection::centroid::lm_optimizer::internals::{
    ModelJacobian, ModelSample, ModelStamp, assert_batch_matches_reference,
    assert_jacobian_matches_differences,
};
use crate::star_detection::centroid::tests::perturbation::Perturbation;

/// One recovery case: render a star of known parameters, optionally spoil the stamp or lie to
/// the fitter about its background, then check what it got back.
///
/// A clean stamp is samples of the fitted model itself, so the fit must return it to the f32
/// rounding of those samples: 2⁻²⁴ ≈ 6e-8 of each value, so of `A + B`, which reaches the
/// parameters relative to the amplitude — an error of `(A + B)/A` times that, carried at up to a
/// few times more (measured: ≤ 4e-7 at A + B ≈ A). [`EXACT`] times `(A + B)/A` bounds it. A spoiled
/// stamp is held to the position's Cramér–Rao bound instead (see [`RecoveryCase::position_bound`]).
#[derive(Debug)]
struct RecoveryCase {
    name: &'static str,
    /// Square stamp side, in pixels.
    stamp: usize,
    center: DVec2,
    amplitude: f32,
    /// Per-axis sigma; equal components render through the circular Gaussian path.
    sigma: Vec2,
    background: f32,
    /// Starting point handed to the fitter, deliberately offset from `center` in some cases.
    guess: DVec2,
    fit_radius: usize,
    perturbation: Perturbation,
    /// Background handed to the fitter. `None` gives it the true one; `Some` deliberately lies
    /// to it, since background is itself a fitted parameter.
    fit_background: Option<f32>,
}

/// The relative error a clean stamp's fitted parameters keep where the star dominates its
/// samples; see [`RecoveryCase`].
const EXACT: f64 = 1e-6;

impl RecoveryCase {
    /// Five standard deviations of the position under the stamp's perturbation: for a Gaussian
    /// profile in white noise σₙ the Fisher information on each centre coordinate is
    /// `(A/σₙ)²·π/2`, whatever its width, so `σ_pos = √(2/π)·σₙ/A`.
    fn position_bound(&self) -> f64 {
        5.0 * (2.0 / PI).sqrt() * f64::from(self.perturbation.rms()) / f64::from(self.amplitude)
    }

    /// Circular sigmas render through the `Gaussian` profile, unequal ones through an axis-aligned
    /// `Elliptical`.
    fn profile(&self) -> StarProfile {
        if self.sigma.x == self.sigma.y {
            StarProfile::Gaussian {
                sigma: self.sigma.x,
            }
        } else {
            StarProfile::Elliptical {
                sigma_x: self.sigma.x,
                sigma_y: self.sigma.y,
                angle: 0.0,
            }
        }
    }
}

const RECOVERY_CASES: &[RecoveryCase] = &[
    RecoveryCase {
        name: "quarter_pixel",
        stamp: 21,
        center: DVec2::new(10.25, 10.25),
        amplitude: 1.0,
        sigma: Vec2::splat(2.5),
        background: 0.1,
        guess: DVec2::splat(10.0),
        fit_radius: 8,
        perturbation: Perturbation::None,
        fit_background: None,
    },
    RecoveryCase {
        name: "half_pixel",
        stamp: 21,
        center: DVec2::new(10.5, 10.5),
        amplitude: 1.0,
        sigma: Vec2::splat(2.5),
        background: 0.1,
        guess: DVec2::splat(10.0),
        fit_radius: 8,
        perturbation: Perturbation::None,
        fit_background: None,
    },
    RecoveryCase {
        name: "three_quarter_pixel",
        stamp: 21,
        center: DVec2::new(10.75, 10.75),
        amplitude: 1.0,
        sigma: Vec2::splat(2.5),
        background: 0.1,
        guess: DVec2::splat(10.0),
        fit_radius: 8,
        perturbation: Perturbation::None,
        fit_background: None,
    },
    RecoveryCase {
        name: "mixed_offset",
        stamp: 21,
        center: DVec2::new(9.7, 10.4),
        amplitude: 1.0,
        sigma: Vec2::splat(2.5),
        background: 0.1,
        guess: DVec2::splat(10.0),
        fit_radius: 8,
        perturbation: Perturbation::None,
        fit_background: None,
    },
    // Fainter than the absolute amplitude floor the fit once had, 0.01 in data units: a faint
    // star in normalized data.
    RecoveryCase {
        name: "below_an_absolute_floor",
        stamp: 21,
        center: DVec2::new(10.3, 9.8),
        amplitude: 0.005,
        sigma: Vec2::splat(2.0),
        background: 0.1,
        guess: DVec2::splat(10.0),
        fit_radius: 8,
        perturbation: Perturbation::None,
        fit_background: None,
    },
    RecoveryCase {
        name: "milli_amplitude",
        stamp: 21,
        center: DVec2::new(10.0, 10.0),
        amplitude: 0.001,
        sigma: Vec2::splat(2.5),
        background: 0.1,
        guess: DVec2::splat(10.0),
        fit_radius: 8,
        perturbation: Perturbation::None,
        fit_background: None,
    },
    RecoveryCase {
        name: "centered",
        stamp: 21,
        center: DVec2::new(10.0, 10.0),
        amplitude: 1.0,
        sigma: Vec2::splat(2.5),
        background: 0.1,
        guess: DVec2::splat(10.0),
        fit_radius: 8,
        perturbation: Perturbation::None,
        fit_background: None,
    },
    RecoveryCase {
        name: "subpixel_offset",
        stamp: 21,
        center: DVec2::new(10.3, 10.7),
        amplitude: 1.0,
        sigma: Vec2::splat(2.5),
        background: 0.1,
        guess: DVec2::new(10.0, 11.0),
        fit_radius: 8,
        perturbation: Perturbation::None,
        fit_background: None,
    },
    RecoveryCase {
        name: "asymmetric",
        stamp: 21,
        center: DVec2::new(10.0, 10.0),
        amplitude: 1.0,
        sigma: Vec2::new(2.0, 3.0),
        background: 0.1,
        guess: DVec2::splat(10.0),
        fit_radius: 8,
        perturbation: Perturbation::None,
        fit_background: None,
    },
    RecoveryCase {
        name: "high_snr",
        stamp: 21,
        center: DVec2::new(10.25, 10.35),
        amplitude: 100.0,
        sigma: Vec2::splat(2.0),
        background: 1.0,
        guess: DVec2::splat(10.0),
        fit_radius: 8,
        perturbation: Perturbation::None,
        fit_background: None,
    },
    RecoveryCase {
        // Amplitude is clamped to a 0.01 floor, so this only pins that a fit comes back at all.
        name: "low_amplitude",
        stamp: 21,
        center: DVec2::new(10.0, 10.0),
        amplitude: 0.05,
        sigma: Vec2::splat(2.5),
        background: 0.1,
        guess: DVec2::splat(10.0),
        fit_radius: 8,
        perturbation: Perturbation::None,
        fit_background: None,
    },
    RecoveryCase {
        name: "large_sigma",
        stamp: 31,
        center: DVec2::new(15.0, 15.0),
        amplitude: 1.0,
        sigma: Vec2::splat(5.0),
        background: 0.1,
        guess: DVec2::splat(15.0),
        fit_radius: 12,
        perturbation: Perturbation::None,
        fit_background: None,
    },
    RecoveryCase {
        // Sigma 1.0 is close to Nyquist: fewer lit pixels, so position is looser.
        name: "small_sigma",
        stamp: 15,
        center: DVec2::new(7.0, 7.0),
        amplitude: 1.0,
        sigma: Vec2::splat(1.0),
        background: 0.1,
        guess: DVec2::splat(7.0),
        fit_radius: 5,
        perturbation: Perturbation::None,
        fit_background: None,
    },
    RecoveryCase {
        name: "zero_background",
        stamp: 21,
        center: DVec2::new(10.0, 10.0),
        amplitude: 1.0,
        sigma: Vec2::splat(2.5),
        background: 0.0,
        guess: DVec2::splat(10.0),
        fit_radius: 8,
        perturbation: Perturbation::None,
        fit_background: None,
    },
    RecoveryCase {
        name: "high_background",
        stamp: 21,
        center: DVec2::new(10.0, 10.0),
        amplitude: 1.0,
        sigma: Vec2::splat(2.5),
        background: 10.0,
        guess: DVec2::splat(10.0),
        fit_radius: 8,
        perturbation: Perturbation::None,
        fit_background: None,
    },
    RecoveryCase {
        name: "sawtooth_noise",
        stamp: 21,
        center: DVec2::new(10.0, 10.0),
        amplitude: 1.0,
        sigma: Vec2::splat(2.5),
        background: 0.1,
        guess: DVec2::splat(10.0),
        fit_radius: 8,
        perturbation: Perturbation::Sawtooth { amplitude: 0.02 },
        fit_background: None,
    },
    RecoveryCase {
        // Noise sigma is 5% of amplitude.
        name: "gaussian_noise",
        stamp: 21,
        center: DVec2::new(10.0, 10.0),
        amplitude: 1.0,
        sigma: Vec2::splat(2.5),
        background: 0.1,
        guess: DVec2::splat(10.0),
        fit_radius: 8,
        perturbation: Perturbation::Gaussian {
            sigma: 0.05,
            seed: 12345,
        },
        fit_background: None,
    },
    RecoveryCase {
        // 15% noise: must still converge, position just gets looser.
        name: "high_noise",
        stamp: 21,
        center: DVec2::new(10.0, 10.0),
        amplitude: 1.0,
        sigma: Vec2::splat(2.5),
        background: 0.1,
        guess: DVec2::splat(10.0),
        fit_radius: 8,
        perturbation: Perturbation::Gaussian {
            sigma: 0.15,
            seed: 54321,
        },
        fit_background: None,
    },
    RecoveryCase {
        // SNR ~0.2. Makes no accuracy claim — it pins only that the fit stays finite instead of
        // diverging, which the runner asserts for every case.
        name: "low_snr",
        stamp: 21,
        center: DVec2::new(10.0, 10.0),
        amplitude: 0.1,
        sigma: Vec2::splat(2.5),
        background: 0.5,
        guess: DVec2::splat(10.0),
        fit_radius: 8,
        perturbation: Perturbation::None,
        fit_background: None,
    },
    RecoveryCase {
        // Fitter is handed a background 20% too high; it should recover the true one anyway,
        // because background is itself a fitted parameter.
        name: "wrong_background_estimate",
        stamp: 21,
        center: DVec2::new(10.0, 10.0),
        amplitude: 1.0,
        sigma: Vec2::splat(2.5),
        background: 0.1,
        guess: DVec2::splat(10.0),
        fit_radius: 8,
        perturbation: Perturbation::None,
        fit_background: Some(0.12),
    },
    RecoveryCase {
        // Guess starts 2px off in both axes and needs the longer budget to walk back.
        name: "bad_initial_guess",
        stamp: 21,
        center: DVec2::new(10.0, 10.0),
        amplitude: 1.0,
        sigma: Vec2::splat(2.5),
        background: 0.1,
        guess: DVec2::new(8.0, 12.0),
        fit_radius: 8,
        perturbation: Perturbation::None,
        fit_background: None,
    },
    RecoveryCase {
        name: "very_high_amplitude",
        stamp: 21,
        center: DVec2::new(10.0, 10.0),
        amplitude: 10000.0,
        sigma: Vec2::splat(2.5),
        background: 100.0,
        guess: DVec2::splat(10.0),
        fit_radius: 8,
        perturbation: Perturbation::None,
        fit_background: None,
    },
    RecoveryCase {
        // Sigma 0.8 is below the pixel scale — barely resolved.
        name: "narrow_psf",
        stamp: 21,
        center: DVec2::new(10.0, 10.0),
        amplitude: 1.0,
        sigma: Vec2::splat(0.8),
        background: 0.1,
        guess: DVec2::splat(10.0),
        fit_radius: 8,
        perturbation: Perturbation::None,
        fit_background: None,
    },
    RecoveryCase {
        // Wide and faint at once: the loosest position bound in the table.
        name: "high_sigma_low_amplitude",
        stamp: 41,
        center: DVec2::new(20.0, 20.0),
        amplitude: 0.1,
        sigma: Vec2::splat(8.0),
        background: 0.05,
        guess: DVec2::splat(20.0),
        fit_radius: 15,
        perturbation: Perturbation::None,
        fit_background: None,
    },
];

#[test]
fn gaussian_fit_recovers_known_parameters() {
    for case in RECOVERY_CASES {
        let mut pixels = SyntheticStar::new(case.center.as_vec2(), case.amplitude, case.profile())
            .stamp(Size2us::new(case.stamp, case.stamp), case.background);
        case.perturbation.apply(&mut pixels);

        let result = GaussianFit::new(
            &pixels,
            case.guess,
            &StampGrid::new(case.fit_radius),
            case.fit_background.unwrap_or(case.background),
            None,
            &GaussianFitConfig::default(),
        )
        .unwrap_or_else(|| panic!("{}: fit returned None", case.name));
        assert!(result.converged, "{}: did not converge", case.name);

        let position_error = (result.pos - case.center).length();
        if matches!(case.perturbation, Perturbation::None) {
            let relative = |got: f32, want: f32| f64::from((got - want).abs() / want);
            let sigma = result.axis_sigma();
            let scale = f64::from(case.amplitude + case.background);
            let bound = EXACT * scale / f64::from(case.amplitude);
            for (what, error) in [
                ("position", position_error),
                ("σx", relative(sigma.x, case.sigma.x)),
                ("σy", relative(sigma.y, case.sigma.y)),
                ("cross term", result.covariance.xy.abs()),
                (
                    "amplitude",
                    relative(result.debug.amplitude, case.amplitude),
                ),
                (
                    "background",
                    f64::from((result.debug.background - case.background).abs())
                        / f64::from(case.amplitude),
                ),
            ] {
                assert!(error <= bound, "{}: {what} off by {error:e}", case.name);
            }
        } else {
            assert!(
                position_error <= case.position_bound(),
                "{}: position off by {position_error} against {}",
                case.name,
                case.position_bound()
            );
        }
    }
}

/// The fit returns `None` where the data cannot support a fit, and lands exactly just inside each
/// bound. A clean σ 2.5 star at the stamp's centre unless a row says otherwise.
#[test]
fn gaussian_fit_rejects_what_the_data_cannot_support() {
    #[derive(Debug)]
    struct Case {
        name: &'static str,
        side: usize,
        /// `None` for a uniform stamp with no star.
        sigma: Option<f32>,
        seed: DVec2,
        radius: usize,
        lands: bool,
    }
    let centre = |side: usize| DVec2::splat((side / 2) as f64);
    let cases = [
        // σ is held to [0.5, radius]: 0.6 and 9 fit, 0.3 and 12 would pin it.
        Case {
            name: "σ just above the floor",
            side: 15,
            sigma: Some(0.6),
            seed: centre(15),
            radius: 5,
            lands: true,
        },
        Case {
            name: "σ under the floor",
            side: 15,
            sigma: Some(0.3),
            seed: centre(15),
            radius: 5,
            lands: false,
        },
        Case {
            name: "σ just under the stamp radius",
            side: 31,
            sigma: Some(9.0),
            seed: centre(31),
            radius: 10,
            lands: true,
        },
        Case {
            name: "σ past the stamp radius",
            side: 41,
            sigma: Some(12.0),
            seed: centre(41),
            radius: 10,
            lands: false,
        },
        // No star: the amplitude falls to its floor.
        Case {
            name: "uniform stamp",
            side: 21,
            sigma: None,
            seed: centre(21),
            radius: 8,
            lands: false,
        },
        // The fit finds the star 2 px from its seed, inside a radius of 3; from 4 px it would
        // have to move beyond it.
        Case {
            name: "centre within the radius of the seed",
            side: 21,
            sigma: Some(2.5),
            seed: DVec2::new(12.0, 10.0),
            radius: 3,
            lands: true,
        },
        Case {
            name: "centre beyond the radius of the seed",
            side: 21,
            sigma: Some(2.5),
            seed: DVec2::new(14.0, 10.0),
            radius: 3,
            lands: false,
        },
        // A stamp that leaves the frame.
        Case {
            name: "stamp off the left edge",
            side: 21,
            sigma: Some(2.5),
            seed: DVec2::new(2.0, 10.0),
            radius: 8,
            lands: false,
        },
        Case {
            name: "stamp larger than the frame",
            side: 5,
            sigma: Some(1.0),
            seed: centre(5),
            radius: 3,
            lands: false,
        },
    ];

    for case in cases {
        let size = Size2us::new(case.side, case.side);
        let truth = centre(case.side);
        let pixels = match case.sigma {
            Some(sigma) => {
                SyntheticStar::new(truth.as_vec2(), 1.0, StarProfile::Gaussian { sigma })
                    .stamp(size, 0.1)
            }
            None => Buffer2::new_filled(case.side, case.side, 0.1),
        };
        let fit = GaussianFit::new(
            &pixels,
            case.seed,
            &StampGrid::new(case.radius),
            0.1,
            None,
            &GaussianFitConfig::default(),
        );
        assert_eq!(fit.is_some(), case.lands, "{}", case.name);
        if let (Some(fit), Some(sigma)) = (fit, case.sigma) {
            assert!(
                (fit.pos - truth).length() <= EXACT,
                "{}: {}",
                case.name,
                fit.pos
            );
            assert!(
                f64::from((fit.axis_sigma().x - sigma).abs() / sigma) <= EXACT,
                "{}: σ {}",
                case.name,
                fit.axis_sigma().x
            );
        }
    }
}

/// The RMS residual a fit reports: the f32 rounding of a clean stamp, and the noise of a noisy one
/// less the seven parameters' share of it, `σₙ·√((N − 7)/N)` over N = 17² pixels. The sample RMS
/// scatters by `1/√(2N)` = 4.2% of σₙ; 5 of those bound it.
#[test]
fn gaussian_fit_rms_residual() {
    let n: f64 = 17.0 * 17.0;
    for noise in [0.0f32, 0.05] {
        let mut pixels =
            SyntheticStar::new(Vec2::splat(10.0), 1.0, StarProfile::Gaussian { sigma: 2.5 })
                .stamp(Size2us::new(21, 21), 0.1);
        patterns::add_gaussian_noise(&mut pixels, noise, 11111);
        let fit = GaussianFit::new(
            &pixels,
            DVec2::splat(10.0),
            &StampGrid::new(8),
            0.1,
            None,
            &GaussianFitConfig::default(),
        )
        .unwrap();
        let rms = f64::from(fit.debug.rms_residual);
        if noise == 0.0 {
            assert!(rms < 1e-7, "clean stamp: {rms}");
        } else {
            let expected = f64::from(noise) * ((n - 7.0) / n).sqrt();
            let bound = 5.0 * f64::from(noise) / (2.0 * n).sqrt();
            assert!(
                (rms - expected).abs() <= bound,
                "noise {noise}: rms {rms} against {expected} ± {bound}"
            );
        }
    }
}

/// The batch normal equations against the scalar reference at every stamp size through 17 — sizes
/// on and off the vector width — with a cross term in play.
#[test]
fn batch_normal_equations_match_reference() {
    let model = Gaussian2D {
        max_sigma: 10.0,
        min_amplitude: 1e-6,
    };
    let truth = [5.0, 5.0, 500.0, 0.25, -0.04, 0.16, 50.0];
    // Away from the truth, so the residuals are not zero.
    let params = [5.2, 4.8, 490.0, 0.23, -0.03, 0.17, 51.0];
    for size in [3, 4, 5, 7, 9, 11, 13, 15, 17] {
        let stamp = ModelStamp::of(&model, size, &truth);
        assert_batch_matches_reference(&model, &stamp, &params);
    }
}

#[test]
fn gaussian_evaluate_and_jacobian_consistency() {
    use crate::star_detection::centroid::lm_optimizer::LMModel;

    let model = Gaussian2D {
        max_sigma: 15.0,
        min_amplitude: 1e-6,
    };
    let params_list: &[[f64; 7]] = &[
        [10.0, 10.0, 1000.0, 0.25, 0.0, 0.25, 100.0],
        [5.5, 7.3, 500.0, 0.44, 0.1, 0.111, 50.0],
        [5.5, 7.3, 500.0, 0.44, -0.2, 0.111, 50.0],
        [0.0, 0.0, 1.0, 1.0, 0.0, 1.0, 0.0],
    ];
    let points = [(8.0, 9.0), (10.0, 10.0), (12.0, 11.0), (5.0, 7.0)];

    for params in params_list {
        for &(x, y) in &points {
            let eval = model.evaluate(x, y, params);
            let jac = Gaussian2D::jacobian_row(x, y, params);
            let ModelSample {
                value: fused_eval,
                jacobian: fused_jac,
            } = model.evaluate_and_jacobian(x, y, params);

            assert!(
                (eval - fused_eval).abs() <= 64.0 * f64::EPSILON * eval.abs().max(1.0),
                "evaluate mismatch: eval={eval}, fused={fused_eval}"
            );
            // The fused form sums the exponent as dx·t + dy·u, the reference as
            // a·dx² + 2b·dx·dy + c·dy²: a few ulps apart, which exp carries into E scaled by the
            // exponent's size (≤ 8 over these points). 64ε relative bounds both.
            for i in 0..7 {
                assert!(
                    (jac[i] - fused_jac[i]).abs() <= 64.0 * f64::EPSILON * jac[i].abs().max(1.0),
                    "jacobian[{i}] mismatch: jac={}, fused={}",
                    jac[i],
                    fused_jac[i]
                );
            }
        }
        assert_jacobian_matches_differences(&model, params, &points);
    }
}

/// The inverse-covariance form has no angle, so a round star is an ordinary point of it: its fit
/// takes no more iterations than an elongated one's from the same round seed.
#[test]
fn gaussian_fit_converges_as_fast_on_a_round_star() {
    let size = Size2us::new(31, 31);
    let iterations = |sigma_x: f32, sigma_y: f32| {
        let pixels = SyntheticStar::new(
            Vec2::new(15.0, 15.0),
            0.8,
            StarProfile::Elliptical {
                sigma_x,
                sigma_y,
                angle: FRAC_PI_4,
            },
        )
        .stamp(size, 0.1);
        let fit = GaussianFit::new(
            &pixels,
            DVec2::new(15.3, 14.8),
            &StampGrid::new(8),
            0.1,
            None,
            &GaussianFitConfig::default(),
        )
        .expect("the fit lands");
        assert!(fit.converged);
        fit.debug.iterations
    };
    let round = iterations(2.5, 2.5);
    let elongated = iterations(3.5, 2.0);
    // Measured: 4 and 5.
    assert!(round <= elongated, "round {round} vs elongated {elongated}");
}
