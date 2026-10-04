//! The `Auto` model ladder: it settles on the simplest model that fits, climbs past a rung the
//! caller's gate would refuse, keeps a rung that fit when a later one fails, and reports every
//! rung's reason when none fits.

use crate::internals::prelude::*;
use crate::internals::synthetic::transforms::{add_star_noise, generate_random_stars};
use crate::registration::distortion::sip::SipConfig;
use crate::registration::ransac::config::RansacConfig;
use crate::registration::register;
use crate::registration::result::RegistrationError;
use crate::registration::tests::helpers::{self, FWHM_TIGHT, map_stars};
use crate::registration::transform::{Transform, TransformModel};
use crate::registration::{Config, TransformType};

/// `s·R(θ)` about (1000, 1000), then `offset`.
fn about_centre(offset: DVec2, angle_deg: f64, scale: f64) -> Transform {
    let linear = Transform::similarity(DVec2::ZERO, angle_deg.to_radians(), scale);
    let centre = DVec2::splat(1000.0);
    Transform::similarity(
        centre + offset - linear.apply(centre),
        angle_deg.to_radians(),
        scale,
    )
}

#[test]
fn auto_ladder_selects_simplest_adequate_model() {
    // `Auto` must accept the fewest-DOF transform within 0.5 px RMS, not overfit. Each ground
    // truth is built so every simpler model genuinely exceeds the threshold.
    let ref_stars = generate_random_stars(90, 2000.0, 2000.0, 101_010, FWHM_TIGHT);
    let config = Config {
        transform_type: TransformModel::Auto,
        matching: helpers::matching_config(8, 6),
        ..Default::default()
    };

    // Rigid (rotation + translation, scale exactly 1.0) → Euclidean, NOT Similarity.
    let euclid = map_stars(&ref_stars, &about_centre(DVec2::new(40.0, -25.0), 0.8, 1.0));
    let result = register(&ref_stars, &euclid, &config).expect("euclidean auto");
    assert_eq!(
        result.transform().transform_type(),
        TransformType::Euclidean,
        "scale-1 rigid set should select Euclidean, got {:?}",
        result.transform().transform_type()
    );

    // Uniform scale ≠ 1 → Euclidean can't fit the scale → Similarity.
    let sim = map_stars(
        &ref_stars,
        &about_centre(DVec2::new(30.0, -20.0), 0.5, 1.002),
    );
    let result = register(&ref_stars, &sim, &config).expect("similarity auto");
    assert_eq!(
        result.transform().transform_type(),
        TransformType::Similarity,
        "uniformly-scaled set should select Similarity, got {:?}",
        result.transform().transform_type()
    );

    // Anisotropic scale (shear-like) → Euclidean/Similarity fail → Affine, NOT Homography.
    let affine = map_stars(
        &ref_stars,
        &Transform::affine([1.003, 0.0, 20.0, 0.0, 0.998, -15.0]),
    );
    let result = register(&ref_stars, &affine, &config).expect("affine auto");
    assert_eq!(
        result.transform().transform_type(),
        TransformType::Affine,
        "anisotropic set should select Affine (not jump to Homography), got {:?}",
        result.transform().transform_type()
    );

    // Perspective → no linear model fits → fall through to Homography.
    let homog = map_stars(
        &ref_stars,
        &Transform::homography([1.0, 0.0, 25.0, 0.0, 1.0, -18.0, 1e-5, 5e-6]),
    );
    let result = register(&ref_stars, &homog, &config).expect("homography auto");
    assert_eq!(
        result.transform().transform_type(),
        TransformType::Homography,
        "perspective set should fall through to Homography, got {:?}",
        result.transform().transform_type()
    );
}

/// The ladder's own bar and the caller's accuracy gate are separate constants judging the same
/// number, so the stricter has to win. A rung landing between them would otherwise be accepted by
/// `auto_ladder` and then rejected by `register`, failing a registration that a later rung
/// satisfies outright.
#[test]
fn a_tight_accuracy_gate_climbs_the_ladder_instead_of_failing_on_a_rung() {
    // Anisotropy of 1.2e-3 across a 2000 px field puts Euclidean at ~0.47 px RMS — under the
    // ladder's 0.5 px bar, over a 0.4 px gate — while Affine fits the same set exactly.
    let ref_stars = generate_random_stars(90, 2000.0, 2000.0, 101_010, FWHM_TIGHT);
    let target = map_stars(
        &ref_stars,
        &Transform::affine([1.0006, 0.0, 20.0, 0.0, 0.9994, -15.0]),
    );
    let config = Config {
        transform_type: TransformModel::Auto,
        matching: helpers::matching_config(8, 6),
        ..Default::default()
    };

    // The default gate (2.0 px) is looser than the bar, so the ladder stops at the simplest model.
    let relaxed = register(&ref_stars, &target, &config).expect("relaxed gate");
    assert_eq!(
        relaxed.transform().transform_type(),
        TransformType::Euclidean
    );
    let euclidean_rms = relaxed.rms_error();
    assert!(
        (0.4..=0.5).contains(&euclidean_rms),
        "fixture must land between the two thresholds to test anything, got {euclidean_rms}"
    );

    // A gate tighter than the bar tightens the bar with it: that rung is now rejected and the
    // ladder climbs to a model the caller will actually accept.
    let strict = register(
        &ref_stars,
        &target,
        &Config {
            max_rms_error: 0.4,
            ..config.clone()
        },
    )
    .expect("a tight gate must climb the ladder, not fail on a rung the gate rejects");
    assert_eq!(strict.transform().transform_type(), TransformType::Affine);
    assert!(
        strict.rms_error() <= 0.4,
        "climbed rung must satisfy the gate, got {}",
        strict.rms_error()
    );
}

/// A rung that fit is the run's, even when a later rung fails.
///
/// The ladder's bar is at most the caller's `max_rms_error`, so a fit between the two is one
/// `register` would accept — discarding it and reporting the last rung's error instead loses a
/// usable alignment and, in the pipeline, drops the frame.
#[test]
fn a_rung_that_fit_survives_a_later_rung_failing() {
    // A tight cluster of stars is poorly conditioned for an 8-DOF model: homography finds an order
    // of magnitude fewer inliers than the simpler rungs, too few for SIP's `3 × terms`, so that
    // rung fails outright while every simpler one fits.
    let ref_stars = generate_random_stars(60, 60.0, 60.0, 999, FWHM_TIGHT);
    let target = add_star_noise(
        &map_stars(&ref_stars, &Transform::translation(DVec2::new(5.0, -3.0))),
        1.4,
        32,
    );
    let config = Config {
        matching: helpers::matching_config(8, 6),
        sip: Some(SipConfig::default()),
        // The fixture is deliberately marginal — 1.4 px of noise against a scorer scale of 0.67 px —
        // so which rungs fit hangs on the samples drawn and the matches made.
        ransac: RansacConfig {
            seed: 2,
            ..Default::default()
        },
        ..Default::default()
    };

    // The fixture's premise: asked for a homography specifically, this pair cannot register.
    let fixed = register(
        &ref_stars,
        &target,
        &Config {
            transform_type: TransformModel::Fixed(TransformType::Homography),
            ..config.clone()
        },
    );
    assert!(
        fixed.is_err(),
        "fixture must fail on Homography to test anything, got {:?}",
        fixed.map(|r| r.rms_error())
    );

    // `Auto` reaches the same failing rung last, and returns the fit it already had.
    let auto = register(
        &ref_stars,
        &target,
        &Config {
            transform_type: TransformModel::Auto,
            ..config
        },
    )
    .expect("a failing top rung must not discard a rung that fit");
    assert_ne!(
        auto.transform().transform_type(),
        TransformType::Homography,
        "the rung that failed cannot be the one returned"
    );
    let rms = auto.rms_error();
    assert!(
        rms > 0.5,
        "a retained rung is one that missed the 0.5 px bar, got {rms}"
    );
    assert!(rms <= 2.0, "and one the default gate accepts, got {rms}");
}

/// When no rung fits, every rung's reason reaches the caller.
///
/// The rungs fail independently — RANSAC estimates the model it is given, and SIP is fit on that
/// model's inlier set — so the last rung's error is not a summary of the rest.
#[test]
fn every_rung_failing_reports_every_reason() {
    // Order 5 needs 3 × 18 = 54 points; 40 stars cannot supply that at any rung.
    let ref_stars = generate_random_stars(40, 2000.0, 2000.0, 5150, FWHM_TIGHT);
    let target = add_star_noise(
        &map_stars(
            &ref_stars,
            &Transform::affine([1.0006, 0.0, 20.0, 0.0, 0.9994, -15.0]),
        ),
        1.2,
        12,
    );
    let config = Config {
        transform_type: TransformModel::Auto,
        matching: helpers::matching_config(8, 6),
        sip: Some(SipConfig {
            order: 5,
            ..Default::default()
        }),
        ransac: RansacConfig {
            seed: 2,
            ..Default::default()
        },
        ..Default::default()
    };

    let error = register(&ref_stars, &target, &config).unwrap_err();
    let RegistrationError::AutoLadderExhausted { failures } = &error else {
        panic!("expected the ladder to report every rung, got {error}");
    };

    assert_eq!(
        failures.iter().map(|rung| rung.model).collect::<Vec<_>>(),
        vec![
            TransformType::Euclidean,
            TransformType::Similarity,
            TransformType::Affine,
            TransformType::Homography,
        ],
        "every rung, in ladder order"
    );

    // Each rung reports its own inlier count, not the last one's: the counts differ because each
    // model admits a different consensus set.
    let found: Vec<usize> = failures
        .iter()
        .map(|rung| match rung.error.as_ref() {
            RegistrationError::InsufficientSipPoints { found, .. } => *found,
            other => panic!("expected a SIP point-count failure, got {other}"),
        })
        .collect();
    assert!(
        found.iter().any(|count| *count != found[0]),
        "rungs that reached SIP with different inlier counts must report their own: {found:?}"
    );

    // And the message carries all four, on one line.
    let message = error.to_string();
    for model in ["Euclidean", "Similarity", "Affine", "Homography"] {
        assert!(message.contains(model), "{model} missing from {message}");
    }
    assert!(!message.contains('\n'), "must stay one line: {message}");
}
