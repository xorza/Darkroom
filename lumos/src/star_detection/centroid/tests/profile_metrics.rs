#![expect(
    clippy::cast_possible_wrap,
    reason = "test fixtures are small images, with non-negative coordinates and offsets of a few dozen pixels"
)]

use super::*;
use crate::star_detection::centroid::stamp::StampFit;

/// A converged fit's widths replace the moment-based FWHM and eccentricity: the Gaussian's both,
/// the Moffat's FWHM only — its single α says nothing of elongation, so eccentricity stays the
/// moments'.
///
/// Each star is a clean sample of the fitted model, measured at the default seed FWHM 4.0 (stamp
/// radius 7), so the fit lands on the truth: every width to [`EXACT_FIT`] of itself, which moves
/// the FWHM — the equal-area √(w₁w₂) for the Gaussian, 2α√(2^(1/β) − 1) for the Moffat — by no
/// more than that, plus ½ε of the f32 it is reported in. Eccentricity √(1 − λ₂/λ₁) is
/// ill-conditioned at 0: two variances each 2·[`EXACT_FIT`] off read as √(4·[`EXACT_FIT`]), 2.1e-3,
/// and the elongated star's √0.75 moves by λ₂/λ₁ · 4·[`EXACT_FIT`] / (2e) = 6.5e-7.
///
/// The radius-7 stamp cuts the wide stars, which biases their moments (the Moffat by 16%) and not
/// the fit, so where the moments disagree with the truth the reported FWHM is the fit's.
#[test]
fn fits_report_their_own_widths() {
    /// [`EXACT_FIT_PX2`]'s relative counterpart on a width, scaled by `(A + B) / A` = 1.125 for the
    /// sky under these stars, as the fit modules' own exact rows are.
    const EXACT_FIT: f64 = 1e-6 * 1.125;
    struct Case {
        name: &'static str,
        profile: StarProfile,
        method: CentroidMethod,
        fwhm: f32,
        /// `None` where the eccentricity is the moments'.
        eccentricity: Option<f64>,
        /// The moments' FWHM is off the truth by more than 1e-3 of it.
        moments_biased: bool,
    }
    let cases = [
        Case {
            name: "round Gaussian",
            profile: StarProfile::Gaussian { sigma: 2.0 },
            method: CentroidMethod::GaussianFit,
            fwhm: sigma_to_fwhm(2.0),
            eccentricity: Some(0.0),
            moments_biased: false,
        },
        Case {
            name: "elongated Gaussian",
            profile: StarProfile::Elliptical {
                sigma_x: 2.0,
                sigma_y: 4.0,
                angle: 0.0,
            },
            method: CentroidMethod::GaussianFit,
            fwhm: sigma_to_fwhm(8.0f32.sqrt()),
            eccentricity: Some(0.75f64.sqrt()),
            moments_biased: true,
        },
        Case {
            name: "Moffat",
            profile: StarProfile::Moffat {
                alpha: 3.0,
                beta: 2.5,
            },
            method: CentroidMethod::MoffatFit { beta: 2.5 },
            fwhm: alpha_beta_to_fwhm(3.0, 2.5),
            eccentricity: None,
            moments_biased: true,
        },
    ];
    let pos = DVec2::splat(64.0);
    for case in cases {
        let pixels =
            SyntheticStar::new(pos.as_vec2(), 0.8, case.profile).stamp(Size2us::new(128, 128), 0.1);
        let measured = Measured::flat(&pixels, 0.1, 0.01);
        let region = measured.region_at(pos);
        let measure = |centroid_method| {
            let config = MeasurementConfig {
                centroid_method,
                ..Default::default()
            };
            measured
                .measure(&region, &config, 4.0)
                .expect("the star measures")
        };
        let star = measure(case.method);
        let moments = measure(CentroidMethod::WeightedMoments);

        let truth = f64::from(case.fwhm);
        let bound = truth * (EXACT_FIT + f64::from(f32::EPSILON) / 2.0);
        let error = (f64::from(star.fwhm) - truth).abs();
        assert!(
            error <= bound,
            "{}: FWHM {} vs {truth}",
            case.name,
            star.fwhm
        );
        let moments_error = (f64::from(moments.fwhm) - truth).abs();
        assert_eq!(
            moments_error > 1e-3 * truth,
            case.moments_biased,
            "{}: moments FWHM {}",
            case.name,
            moments.fwhm
        );

        let eccentricity = f64::from(star.eccentricity);
        match case.eccentricity {
            Some(0.0) => assert!(
                eccentricity <= (4.0 * EXACT_FIT).sqrt(),
                "{}: eccentricity {eccentricity}",
                case.name
            ),
            Some(expected) => assert!(
                (eccentricity - expected).abs() <= 6.5e-7 + f64::from(f32::EPSILON),
                "{}: eccentricity {eccentricity} vs {expected}",
                case.name
            ),
            None => assert_eq!(star.eccentricity, moments.eccentricity, "{}", case.name),
        }
    }
}

/// The windowed covariance deconvolves its Gaussian window, `C = (C_obs⁻¹ − σ_w⁻²·I)⁻¹`, which is
/// exact for a Gaussian source: it returns the source's own covariance, axes kept apart, from any
/// seed window — one pass already lands, and the rest re-weight by the same answer.
///
/// The stamps keep ≥ 4.67σ of each source along every axis: what they cut and the sampling leave
/// ≤ 3.1e-8 of each variance (measured), under the 1e-7 asserted.
#[test]
fn windowed_covariance_deconvolves_to_the_source() {
    let size = Size2us::new(64, 64);
    let pos = DVec2::splat(32.0);
    for (sigma_x, sigma_y, radius) in [(2.5f32, 2.5f32, 12), (3.0, 2.0, 14), (2.0, 3.0, 14)] {
        let pixels = SyntheticStar::new(
            pos.as_vec2(),
            1.0,
            StarProfile::Elliptical {
                sigma_x,
                sigma_y,
                angle: 0.0,
            },
        )
        .stamp(size, 0.0);
        let measured = Measured::flat(&pixels, 0.0, 1.0);
        let (xx, yy) = (f64::from(sigma_x).powi(2), f64::from(sigma_y).powi(2));
        for seed in [1.0, f64::midpoint(xx, yy), 4.0 * xx.max(yy)] {
            let cov = windowed_covariance(&measured.residual, 0.0, pos, radius, seed)
                .expect("a clean Gaussian converges");
            let name = format!("σ ({sigma_x}, {sigma_y}) from seed {seed}");
            assert!((cov.xx / xx - 1.0).abs() <= 1e-7, "{name}: xx {}", cov.xx);
            assert!((cov.yy / yy - 1.0).abs() <= 1e-7, "{name}: yy {}", cov.yy);
            assert!(cov.xy.abs() <= 1e-7 * xx.min(yy), "{name}: xy {}", cov.xy);
        }
    }
}

/// Noise in the wings is what the window is for. At the matched window `σ_w = σ` the weighted moments
/// are σ²/2 per axis and deconvolve back to σ², so noise n moves the axis ratio √(yy/xx) by
/// `2 Σ wᵢ(fyᵢ² − fxᵢ²)nᵢ / (σ² Σ wᵢIᵢ)`, a scatter of `2σₙ √Σ wᵢ²(fyᵢ² − fxᵢ²)² / (σ² Σ wᵢIᵢ)` =
/// 0.0135 on this star. Plain signed moments over the same stamp scatter by
/// `σₙ √Σ(fyᵢ² − fxᵢ²)² / (2σ² Σ Iᵢ)` = 0.100, 7.4 times as much — the failure mode that inflated
/// the eccentricity of round stars. The one draw here lands within 4 of the window's scatter, which
/// plain moments would miss for most draws.
#[test]
fn windowed_covariance_holds_wing_noise_to_its_propagated_scatter() {
    const NOISE: f32 = 0.03;
    let size = Size2us::new(64, 64);
    let pos = DVec2::splat(32.0);
    let sigma = 2.5f32;
    let radius = 12;
    let mut pixels =
        SyntheticStar::new(pos.as_vec2(), 1.0, StarProfile::Gaussian { sigma }).stamp(size, 0.1);
    patterns::add_gaussian_noise(pixels.pixels_mut(), NOISE, 12345);
    let measured = Measured::flat(&pixels, 0.1, NOISE);

    let sigma_sq = f64::from(sigma).powi(2);
    let mut spread = 0.0;
    let mut weighted_signal = 0.0;
    let arms = -(radius as i32)..=radius as i32;
    for dy in arms.clone() {
        for dx in arms.clone() {
            let weight = (-f64::from(dx * dx + dy * dy) / (2.0 * sigma_sq)).exp();
            spread += (weight * f64::from(dy * dy - dx * dx)).powi(2);
            weighted_signal += weight * weight;
        }
    }
    let scatter = 2.0 * f64::from(NOISE) * spread.sqrt() / (sigma_sq * weighted_signal);

    let cov = windowed_covariance(&measured.residual, 0.0, pos, radius, sigma_sq)
        .expect("a noisy Gaussian converges");
    let ratio = (cov.yy / cov.xx).sqrt();
    assert!(
        (ratio - 1.0).abs() <= 4.0 * scatter,
        "axis ratio {ratio}, scatter {scatter}"
    );
}

#[test]
fn inverse_variance_weights_downweight_bright_pixels() {
    // CCD per-pixel variance = signal/G + sky² + (read_e/G)², G = e-/normalized unit.
    let bg = 0.1;
    let sky_noise = 0.02; // sky_var = 4e-4
    let noise = FitNoise {
        sky_noise,
        noise_model: NoiseModel::from_normalized(1_000.0, 10.0),
    };
    let data_z = [0.1, 0.6, 1.1]; // signals 0.0, 0.5, 1.0

    let w: Vec<f64> = data_z.iter().map(|&z| noise.weight(z, bg)).collect();

    // signal 0.0: 1/(0      + 4e-4 + 1e-4) = 2000
    // signal 0.5: 1/(5e-4   + 5e-4)        = 1000
    // signal 1.0: 1/(1e-3   + 5e-4)        ≈ 666.67
    //
    // Within 1e-3, not 1e-9: `sky_noise` is f32 here because that is what production carries, so
    // 0.02 quantizes and sky_var lands just under a clean 4e-4, moving w0 by 7e-5. The tolerance
    // is still 5e-7 relative.
    assert!((w[0] - 2000.0).abs() < 1e-3, "w0 = {}", w[0]);
    assert!((w[1] - 1000.0).abs() < 1e-3, "w1 = {}", w[1]);
    assert!((w[2] - 666.666_666_666_666_6).abs() < 1e-3, "w2 = {}", w[2]);
    assert!(
        w[0] > w[1] && w[1] > w[2],
        "weight must fall as signal rises"
    );

    // The same weights reach the fit through `prepare`'s single pass, which is the only production
    // path to them — a flat stamp above the sky must weigh every pixel identically.
    let pixels = Buffer2::new_filled(32, 32, 0.6f32);
    let fit = StampFit::prepare::<6>(
        &pixels,
        DVec2::splat(16.0),
        &StampGrid::new(3),
        bg as f32,
        Some(noise),
    )
    .expect("stamp at centre");
    let weights = fit.weights.expect("a noise model means a weighted fit");
    assert_eq!(weights.len(), 7 * 7);
    assert!(
        weights.iter().all(|&v| (v - 1000.0).abs() < 1e-3),
        "{weights:?}"
    );
}
