//! The `Auto` model choice by GRIC: it settles on the fewest parameters that fit, takes no model
//! for a blend, honours the caller's accuracy gate, keeps a model that fit when another fails, and
//! reports every model's reason when none fits.

use crate::internals::prelude::*;
use crate::internals::synthetic::transforms::{add_star_noise, generate_random_stars};
use crate::registration::distortion::sip::SipConfig;
use crate::registration::ransac::config::RansacConfig;
use crate::registration::register;
use crate::registration::result::RegistrationError;
use crate::registration::tests::helpers::{self, FWHM_TIGHT, map_stars};
use crate::registration::transform::{Transform, TransformModel};
use crate::registration::{RegistrationConfig, TransformType};
use crate::star_detection::star::Star;

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

/// Each truth fits exactly with its own model and leaves every simpler one pixels off on stars of
/// position σ 0.01 px, so its residuals' caps outweigh the parameters a more general model costs:
/// GRIC takes the truth's model, and none more general.
#[test]
fn auto_selects_the_simplest_adequate_model() {
    let ref_stars = generate_random_stars(90, 2000.0, 2000.0, 101_010, FWHM_TIGHT);
    let config = RegistrationConfig {
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

    // Perspective → no linear model fits → Homography.
    let homog = map_stars(
        &ref_stars,
        &Transform::homography([1.0, 0.0, 25.0, 0.0, 1.0, -18.0, 1e-5, 5e-6]),
    );
    let result = register(&ref_stars, &homog, &config).expect("homography auto");
    assert_eq!(
        result.transform().transform_type(),
        TransformType::Homography,
        "perspective set should select Homography, got {:?}",
        result.transform().transform_type()
    );
}

/// One blend among 50 true pairs buys no model. 50 stars under a rotation, exact to their
/// position σ of 0.01 px, and one more whose target sits 4 px off its true place: the robust
/// fit's gate drops the blend's pair, the Euclidean fit is exact on the rest, and GRIC takes it.
/// A least-squares ladder read 0.57 px RMS from the blend and climbed to an affine map.
#[test]
fn a_blend_buys_no_model() {
    let ref_stars = generate_random_stars(51, 2000.0, 2000.0, 4242, FWHM_TIGHT);
    let truth = about_centre(DVec2::new(12.0, -5.0), 0.4, 1.0);
    let mut target = map_stars(&ref_stars, &truth);
    target[50].pos += DVec2::new(4.0, 0.0);
    let config = RegistrationConfig {
        transform_type: TransformModel::Auto,
        matching: helpers::matching_config(8, 6),
        ..Default::default()
    };
    let result = register(&ref_stars, &target, &config).expect("registers");
    assert_eq!(
        result.transform().transform_type(),
        TransformType::Euclidean
    );
    assert_eq!(result.num_inliers(), 50);
    assert!(
        result
            .matched_stars()
            .iter()
            .all(|m| m.indices.reference != 50)
    );
    for p in [DVec2::ZERO, DVec2::splat(2000.0)] {
        assert!(result.transform().apply(p).distance(truth.apply(p)) <= 1e-9);
    }
}

/// GRIC weighs fit against degrees of freedom; the caller's accuracy gate is a requirement. On 90
/// stars of position σ 0.5 px, an anisotropy of 5e-4 across 2000 px leaves the Euclidean fit about
/// 0.2 px RMS, `z²` ≈ 0.08 a pair against the affine map's exact fit: 90·0.08 + 3·ln 360 ≈ 25 for
/// Euclidean, 6·ln 360 ≈ 35 for the affine map, so GRIC takes the simpler. A gate of 0.15 px
/// refuses it while the affine map passes, and the affine map is the answer.
#[test]
fn a_tight_accuracy_gate_is_a_requirement_gric_honours() {
    let ref_stars: Vec<Star> = generate_random_stars(90, 2000.0, 2000.0, 101_010, FWHM_TIGHT)
        .into_iter()
        .map(|star| Star {
            position_sigma: 0.5,
            ..star
        })
        .collect();
    let target = map_stars(
        &ref_stars,
        &Transform::affine([1.000_25, 0.0, 20.0, 0.0, 0.999_75, -15.0]),
    );
    let config = RegistrationConfig {
        transform_type: TransformModel::Auto,
        matching: helpers::matching_config(8, 6),
        ..Default::default()
    };

    let relaxed = register(&ref_stars, &target, &config).expect("relaxed gate");
    assert_eq!(
        relaxed.transform().transform_type(),
        TransformType::Euclidean
    );
    let euclidean_rms = relaxed.rms_error();
    assert!(
        (0.15..=0.3).contains(&euclidean_rms),
        "the fixture must leave the Euclidean fit past the strict gate, got {euclidean_rms}"
    );

    let strict = register(
        &ref_stars,
        &target,
        &RegistrationConfig {
            max_rms_error: 0.15,
            ..config.clone()
        },
    )
    .expect("a tight gate must take the model that meets it, not fail on the one GRIC prefers");
    assert_eq!(strict.transform().transform_type(), TransformType::Affine);
    assert!(strict.rms_error() <= 1e-9, "{}", strict.rms_error());
}

/// A model that fit is a candidate, even when another fails: reporting the failure instead would
/// lose a usable alignment and, in the pipeline, drop the frame.
#[test]
fn a_model_that_fit_survives_another_failing() {
    // A tight, noisy cluster leaves the affine map with SIP fewer pairs than SIP's `3 × terms`, 20
    // of 21 (measured), so it fails outright while the rotation and the similarity fit.
    let ref_stars = generate_random_stars(60, 60.0, 60.0, 999, FWHM_TIGHT);
    let target = add_star_noise(
        &map_stars(&ref_stars, &Transform::translation(DVec2::new(5.0, -3.0))),
        1.4,
        32,
    );
    let config = RegistrationConfig {
        matching: helpers::matching_config(8, 6),
        sip: Some(SipConfig::default()),
        // The fixture is deliberately marginal — 1.4 px of noise against a scorer scale of 0.67 px —
        // so which models fit hangs on the samples drawn and the matches made.
        ransac: RansacConfig {
            seed: 2,
            ..Default::default()
        },
        ..Default::default()
    };

    // The fixture's premise: asked for an affine map specifically, this pair cannot register.
    let fixed = register(
        &ref_stars,
        &target,
        &RegistrationConfig {
            transform_type: TransformModel::Fixed(TransformType::Affine),
            ..config.clone()
        },
    );
    assert!(
        matches!(fixed, Err(RegistrationError::InsufficientSipPoints { .. })),
        "fixture must fail on Affine to test anything, got {:?}",
        fixed.map(|r| r.rms_error())
    );

    // `Auto` meets the same failure, and chooses among the models that fit.
    let auto = register(
        &ref_stars,
        &target,
        &RegistrationConfig {
            transform_type: TransformModel::Auto,
            ..config
        },
    )
    .expect("a failing model must not discard the models that fit");
    assert_ne!(
        auto.transform().transform_type(),
        TransformType::Affine,
        "the model that failed cannot be the one returned"
    );
    let rms = auto.rms_error();
    assert!(rms <= 2.0, "the default gate accepts it, got {rms}");
}

/// When no model fits, every model's reason reaches the caller: the models fail independently —
/// RANSAC estimates the model it is given — so no one error is a summary of the rest. Under a SIP
/// correction the candidates stop at the affine map.
#[test]
fn every_model_failing_reports_every_reason() {
    // Order 5 needs 3 × 18 = 54 points; 40 stars cannot supply that to any model.
    let ref_stars = generate_random_stars(40, 2000.0, 2000.0, 5150, FWHM_TIGHT);
    let target = add_star_noise(
        &map_stars(
            &ref_stars,
            &Transform::affine([1.0006, 0.0, 20.0, 0.0, 0.9994, -15.0]),
        ),
        1.2,
        12,
    );
    let config = RegistrationConfig {
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
    let RegistrationError::EveryModelFailed { failures } = &error else {
        panic!("expected every model reported, got {error}");
    };

    assert_eq!(
        failures
            .iter()
            .map(|failure| failure.model)
            .collect::<Vec<_>>(),
        vec![
            TransformType::Euclidean,
            TransformType::Similarity,
            TransformType::Affine,
        ],
        "every model a SIP correction takes, from the fewest degrees of freedom"
    );

    // Each model reports its own pair count, not the last one's: the counts differ because each
    // model admits a different consensus set.
    let found: Vec<usize> = failures
        .iter()
        .map(|failure| match failure.error.as_ref() {
            RegistrationError::InsufficientSipPoints { found, .. } => *found,
            other => panic!("expected a SIP point-count failure, got {other}"),
        })
        .collect();
    assert!(
        found.iter().any(|count| *count != found[0]),
        "models that reached SIP with different pair counts must report their own: {found:?}"
    );

    // And the message carries all three, on one line.
    let message = error.to_string();
    for model in ["Euclidean", "Similarity", "Affine"] {
        assert!(message.contains(model), "{model} missing from {message}");
    }
    assert!(!message.contains('\n'), "must stay one line: {message}");
}
