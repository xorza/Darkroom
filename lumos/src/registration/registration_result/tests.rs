use glam::DVec2;

use crate::registration::error::{
    RansacFailure, RegistrationCatalog, RegistrationError, StarField,
};
use crate::registration::match_indices::MatchIndices;
use crate::registration::registration_result::{RegistrationResult, StarMatch, rms_error};
use crate::registration::transform::Transform;
use crate::registration::warp_transform::WarpTransform;

/// The result of `transform` fitted to `matches`.
fn result(transform: Transform, matches: Vec<StarMatch>) -> RegistrationResult {
    let rms = rms_error(&matches);
    RegistrationResult::new(WarpTransform::new(transform), matches, rms)
}

fn identity_matches(residuals: &[f64]) -> Vec<StarMatch> {
    residuals
        .iter()
        .enumerate()
        .map(|(index, &residual)| StarMatch {
            indices: MatchIndices {
                reference: index,
                target: index,
            },
            residual,
        })
        .collect()
}

#[test]
fn result_keeps_matches_and_derives_diagnostics() {
    let transform = Transform::translation(DVec2::new(1.0, 2.0));
    let matches = vec![
        StarMatch {
            indices: MatchIndices {
                reference: 0,
                target: 2,
            },
            residual: 0.1,
        },
        StarMatch {
            indices: MatchIndices {
                reference: 1,
                target: 4,
            },
            residual: 0.2,
        },
        StarMatch {
            indices: MatchIndices {
                reference: 3,
                target: 5,
            },
            residual: 0.15,
        },
    ];

    let result = result(transform, matches.clone());

    assert_eq!(result.transform().matrix(), transform.matrix());
    assert!(result.sip().is_none());
    assert_eq!(result.matched_stars(), matches);
    assert_eq!(result.num_inliers(), 3);

    // sqrt((0.1² + 0.2² + 0.15²) / 3) = sqrt(0.0725 / 3).
    let expected_rms = (0.0725_f64 / 3.0).sqrt();
    assert_eq!(result.rms_error().to_bits(), expected_rms.to_bits());
    // exp(−rms / 2) · 3 / 20, the same operations in the same order.
    assert_eq!(
        result.quality_score(),
        (-expected_rms / 2.0).exp() * (3.0 / 20.0)
    );
}

#[test]
fn quality_score_uses_error_and_saturating_inlier_factors() {
    let four_residuals = [0.1, 0.2, 0.15, 0.05];
    let four = result(Transform::identity(), identity_matches(&four_residuals));

    // RMS = sqrt((0.01 + 0.04 + 0.0225 + 0.0025) / 4).
    let expected_rms = (0.075_f64 / 4.0).sqrt();
    let expected_quality = (-expected_rms / 2.0).exp() * (4.0 / 20.0);
    assert!((four.rms_error() - expected_rms).abs() < f64::EPSILON);
    assert!((four.quality_score() - expected_quality).abs() < f64::EPSILON);

    let twenty_five = result(Transform::identity(), identity_matches(&[0.1; 25]));
    // The inlier factor saturates at 20, so quality = exp(-0.1 / 2).
    let saturated_quality = (-0.05_f64).exp();
    assert!((twenty_five.rms_error() - 0.1).abs() < f64::EPSILON);
    assert!((twenty_five.quality_score() - saturated_quality).abs() < f64::EPSILON);
}

/// The result hands out the warp it was built from, borrowed or taken whole.
#[test]
fn the_result_hands_out_its_warp() {
    let transform = Transform::translation(DVec2::new(5.0, -3.0));
    let result = result(transform, identity_matches(&[0.1, 0.2, 0.15, 0.05, 0.12]));
    let warp = result.warp_transform();
    assert!(!warp.has_sip());
    assert_eq!(warp.apply(DVec2::new(10.0, 20.0)), DVec2::new(15.0, 17.0));
    let warp = result.into_warp();
    assert_eq!(warp.apply(DVec2::new(10.0, 20.0)), DVec2::new(15.0, 17.0));
}

#[test]
fn registration_error_messages_include_context() {
    let cases = [
        (
            RegistrationError::TooFewStars {
                found: 5,
                required: 10,
            },
            "too few stars detected: found 5, need 10",
        ),
        (
            RegistrationError::NoMatchingPatterns,
            "no matching star patterns found between images",
        ),
        (
            RegistrationError::RansacFailed(RansacFailure {
                iterations: 1000,
                best_inlier_count: 0,
            }),
            "RANSAC found no inliers (iterations: 1000, best inlier count: 0)",
        ),
        (
            RegistrationError::AccuracyTooLow {
                rms_error: 5.123,
                max_allowed: 2.0,
            },
            "registration accuracy too low: 5.123 pixels (max: 2.000)",
        ),
        (
            RegistrationError::TooFewSipPoints {
                found: 8,
                required: 9,
            },
            "too few points for a SIP fit: found 8, need 9",
        ),
        // The two that interpolate a field's *fields* rather than the field itself, and so are
        // the ones a reformatting of this enum is most likely to get wrong.
        (
            RegistrationError::InvalidStarPosition {
                catalog: RegistrationCatalog::Reference,
                index: 3,
                position: DVec2::new(f64::NAN, 4.5),
            },
            "reference star 3 position must be finite, got (NaN, 4.5)",
        ),
        (
            RegistrationError::InvalidStarField {
                catalog: RegistrationCatalog::Target,
                index: 7,
                field: StarField::Fwhm,
                value: f64::INFINITY,
            },
            "target star 7 FWHM must be finite, got inf",
        ),
    ];

    for (error, expected) in cases {
        assert_eq!(error.to_string(), expected);
    }
}
