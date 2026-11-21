//! The `Auto` model choice by GRIC: it settles on the fewest parameters that fit, takes no model
//! for a blend, honours the caller's accuracy gate, keeps a model that fit when another fails, and
//! reports every model's reason when none fits.

use glam::DVec2;

use crate::internals::synthetic::transforms::{StarScatter, add_star_noise};
use crate::registration::distortion::sip::SipConfig;
use crate::registration::distortion::sip::sip_order::SipOrder;
use crate::registration::error::{FailedModel, ModelFailure, RegistrationError};
use crate::registration::ransac::ransac_config::RansacConfig;
use crate::registration::registrar::Registrar;
use crate::registration::registration_config::RegistrationConfig;
use crate::registration::tests::helpers::{self, FWHM_TIGHT, FieldMotion, map_stars};
use crate::registration::transform::linear_model::LinearModel;
use crate::registration::transform::{Transform, TransformType};
use crate::registration::transform_model::{ModelChoice, TransformModel};
use crate::star_detection::star::Star;

/// Each truth fits exactly with its own model and leaves every simpler one pixels off on stars of
/// position σ 0.01 px, so its residuals' caps outweigh the parameters a more general model costs:
/// GRIC takes the truth's model, and none more general.
#[test]
fn auto_selects_the_simplest_adequate_model() {
    let ref_stars = StarScatter {
        count: 90,
        side: 2000.0,
        seed: 101_010,
    }
    .stars(FWHM_TIGHT);
    let registrar = Registrar::new(RegistrationConfig {
        model: TransformModel::default(),
        matching: helpers::matching_config(6),
        ..Default::default()
    });
    let reference = registrar.reference(ref_stars.clone()).unwrap();

    // Rigid (rotation + translation, scale exactly 1.0) → Euclidean, NOT Similarity.
    let euclid = map_stars(
        &ref_stars,
        &FieldMotion {
            offset: DVec2::new(40.0, -25.0),
            angle_deg: 0.8,
            scale: 1.0,
        }
        .about(1000.0),
    );
    let result = reference.register(&euclid).expect("euclidean auto");
    assert_eq!(
        result.warp().linear().transform_type(),
        TransformType::Euclidean,
        "scale-1 rigid set should select Euclidean, got {:?}",
        result.warp().linear().transform_type()
    );

    // Uniform scale ≠ 1 → Euclidean can't fit the scale → Similarity.
    let sim = map_stars(
        &ref_stars,
        &FieldMotion {
            offset: DVec2::new(30.0, -20.0),
            angle_deg: 0.5,
            scale: 1.002,
        }
        .about(1000.0),
    );
    let result = reference.register(&sim).expect("similarity auto");
    assert_eq!(
        result.warp().linear().transform_type(),
        TransformType::Similarity,
        "uniformly-scaled set should select Similarity, got {:?}",
        result.warp().linear().transform_type()
    );

    // Anisotropic scale (shear-like) → Euclidean/Similarity fail → Affine, NOT Homography.
    let affine = map_stars(
        &ref_stars,
        &Transform::affine([1.003, 0.0, 20.0, 0.0, 0.998, -15.0]),
    );
    let result = reference.register(&affine).expect("affine auto");
    assert_eq!(
        result.warp().linear().transform_type(),
        TransformType::Affine,
        "anisotropic set should select Affine (not jump to Homography), got {:?}",
        result.warp().linear().transform_type()
    );

    // Perspective → no linear model fits → Homography.
    let homog = map_stars(
        &ref_stars,
        &Transform::homography([1.0, 0.0, 25.0, 0.0, 1.0, -18.0, 1e-5, 5e-6]),
    );
    let result = reference.register(&homog).expect("homography auto");
    assert_eq!(
        result.warp().linear().transform_type(),
        TransformType::Homography,
        "perspective set should select Homography, got {:?}",
        result.warp().linear().transform_type()
    );
}

/// One blend among 50 true pairs buys no model. 50 stars under a rotation, exact to their
/// position σ of 0.01 px, and one more whose target sits 4 px off its true place: the robust
/// fit's gate drops the blend's pair, the Euclidean fit is exact on the rest, and GRIC takes it.
/// A least-squares ladder read 0.57 px RMS from the blend and climbed to an affine map.
#[test]
fn a_blend_buys_no_model() {
    let ref_stars = StarScatter {
        count: 51,
        side: 2000.0,
        seed: 4242,
    }
    .stars(FWHM_TIGHT);
    let truth = FieldMotion {
        offset: DVec2::new(12.0, -5.0),
        angle_deg: 0.4,
        scale: 1.0,
    }
    .about(1000.0);
    let mut target = map_stars(&ref_stars, &truth);
    target[50].pos += DVec2::new(4.0, 0.0);
    let config = RegistrationConfig {
        model: TransformModel::default(),
        matching: helpers::matching_config(6),
        ..Default::default()
    };
    let result = Registrar::new(config)
        .register(&ref_stars, &target)
        .expect("registers");
    assert_eq!(
        result.warp().linear().transform_type(),
        TransformType::Euclidean
    );
    assert_eq!(result.matches().len(), 50);
    assert!(result.matches().iter().all(|m| m.idxs.reference != 50));
    for p in [DVec2::ZERO, DVec2::splat(2000.0)] {
        assert!(result.warp().linear().apply(p).distance(truth.apply(p)) <= 1e-9);
    }
}

/// GRIC weighs fit against degrees of freedom; the caller's accuracy gate is a requirement. On 90
/// stars of position σ 0.5 px, an anisotropy of 5e-4 across 2000 px leaves the Euclidean fit about
/// 0.2 px RMS, `z²` ≈ 0.08 a pair against the affine map's exact fit: 90·0.08 + 3·ln 360 ≈ 25 for
/// Euclidean, 6·ln 360 ≈ 35 for the affine map, so GRIC takes the simpler. A gate of 0.15 px
/// refuses it while the affine map passes, and the affine map is the answer.
#[test]
fn a_tight_accuracy_gate_is_a_requirement_gric_honours() {
    let ref_stars: Vec<Star> = StarScatter {
        count: 90,
        side: 2000.0,
        seed: 101_010,
    }
    .stars(FWHM_TIGHT)
    .into_iter()
    .map(|star| Star {
        pos_sigma: 0.5,
        ..star
    })
    .collect();
    let target = map_stars(
        &ref_stars,
        &Transform::affine([1.000_25, 0.0, 20.0, 0.0, 0.999_75, -15.0]),
    );
    let config = RegistrationConfig {
        model: TransformModel::default(),
        matching: helpers::matching_config(6),
        ..Default::default()
    };

    let relaxed = Registrar::new(config.clone())
        .register(&ref_stars, &target)
        .expect("relaxed gate");
    assert_eq!(
        relaxed.warp().linear().transform_type(),
        TransformType::Euclidean
    );
    let euclidean_rms = relaxed.rms_error_px();
    assert!(
        (0.15..=0.3).contains(&euclidean_rms),
        "the fixture must leave the Euclidean fit past the strict gate, got {euclidean_rms}"
    );

    let strict = Registrar::new(RegistrationConfig {
        max_rms_error_px: 0.15,
        ..config
    })
    .register(&ref_stars, &target)
    .expect("a tight gate must take the model that meets it, not fail on the one GRIC prefers");
    assert_eq!(
        strict.warp().linear().transform_type(),
        TransformType::Affine
    );
    assert!(strict.rms_error_px() <= 1e-9, "{}", strict.rms_error_px());
}

/// A model that fit is a candidate, even when another fails: reporting the failure instead would
/// lose a usable alignment and, in the pipeline, drop the frame.
#[test]
fn a_model_that_fit_survives_another_failing() {
    // A tight, noisy cluster leaves the affine map with SIP fewer pairs than SIP's `3 × terms`, 20
    // of 21 (measured), so it fails outright while the rotation and the similarity fit.
    let ref_stars = StarScatter {
        count: 60,
        side: 60.0,
        seed: 999,
    }
    .stars(FWHM_TIGHT);
    let target = add_star_noise(
        &map_stars(&ref_stars, &Transform::translation(DVec2::new(5.0, -3.0))),
        1.4,
        32,
    );
    let config = RegistrationConfig {
        model: TransformModel::WithSip(ModelChoice::Auto, SipConfig::default()),
        matching: helpers::matching_config(6),
        // The fixture is deliberately marginal — 1.4 px of noise against a scorer scale of 0.67 px —
        // so which models fit hangs on the samples drawn and the matches made.
        ransac: RansacConfig {
            seed: 1,
            ..Default::default()
        },
        ..Default::default()
    };

    // The fixture's premise: asked for an affine map under the same SIP correction, this pair
    // cannot register.
    let fixed = Registrar::new(RegistrationConfig {
        model: TransformModel::WithSip(
            ModelChoice::Fixed(LinearModel::Affine),
            SipConfig::default(),
        ),
        ..config.clone()
    })
    .register(&ref_stars, &target);
    assert!(
        matches!(fixed, Err(RegistrationError::TooFewSipPoints { .. })),
        "fixture must fail on Affine to test anything, got {:?}",
        fixed.map(|r| r.rms_error_px())
    );

    // `Auto` meets the same failure, and chooses among the models that fit.
    let auto = Registrar::new(RegistrationConfig {
        model: TransformModel::default(),
        ..config
    })
    .register(&ref_stars, &target)
    .expect("a failing model must not discard the models that fit");
    assert_ne!(
        auto.warp().linear().transform_type(),
        TransformType::Affine,
        "the model that failed cannot be the one returned"
    );
    let rms = auto.rms_error_px();
    assert!(rms <= 2.0, "the default gate accepts it, got {rms}");
}

/// A frame too sparse for its SIP correction registers by a plain model: order 5 needs 3 × 18 =
/// 54 pairs, which 40 stars cannot supply, so each corrected candidate is one failed candidate and
/// the plain models compete, every star matched.
#[test]
fn a_frame_too_sparse_for_its_sip_registers_by_a_plain_model() {
    let ref_stars = StarScatter {
        count: 40,
        side: 2000.0,
        seed: 5150,
    }
    .stars(FWHM_TIGHT);
    let target = add_star_noise(
        &map_stars(
            &ref_stars,
            &Transform::affine([1.0006, 0.0, 20.0, 0.0, 0.9994, -15.0]),
        ),
        1.2,
        12,
    );
    let config = RegistrationConfig {
        model: TransformModel::WithSip(
            ModelChoice::Auto,
            SipConfig {
                order: SipOrder::Five,
                ..Default::default()
            },
        ),
        matching: helpers::matching_config(6),
        ransac: RansacConfig {
            seed: 2,
            ..Default::default()
        },
        ..Default::default()
    };
    let result = Registrar::new(config)
        .register(&ref_stars, &target)
        .expect("the plain models fit");
    assert_eq!(result.matches().len(), 40);
    let warp = result.into_warp();
    assert!(warp.sip().is_none(), "{warp:?}");
}

/// When no model fits, every model's reason reaches the caller, each with its own count, on one
/// line.
#[test]
fn every_model_failing_reports_every_reason() {
    let error = RegistrationError::EveryModelFailed {
        failures: [
            (TransformType::Euclidean, 31),
            (TransformType::Similarity, 33),
            (TransformType::Affine, 35),
        ]
        .map(|(model, found)| FailedModel {
            model,
            failure: ModelFailure::TooFewSipPoints {
                found,
                required: 54,
            },
        })
        .to_vec(),
    };
    let message = error.to_string();
    for (model, found) in [("Euclidean", "31"), ("Similarity", "33"), ("Affine", "35")] {
        assert!(message.contains(model), "{model} missing from {message}");
        assert!(message.contains(found), "{found} missing from {message}");
    }
    assert!(!message.contains('\n'), "must stay one line: {message}");
}
