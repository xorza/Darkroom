use glam::DVec2;

use crate::stacking::registration::result::{
    RansacFailureReason, RegistrationError, RegistrationResult, StarMatch,
};
use crate::stacking::registration::transform::Transform;

fn identity_matches(residuals: &[f64]) -> Vec<StarMatch> {
    residuals
        .iter()
        .enumerate()
        .map(|(index, &residual)| StarMatch {
            reference: index,
            target: index,
            residual,
        })
        .collect()
}

#[test]
fn result_keeps_matches_and_derives_diagnostics() {
    let transform = Transform::translation(DVec2::new(1.0, 2.0));
    let matches = vec![
        StarMatch {
            reference: 0,
            target: 2,
            residual: 0.1,
        },
        StarMatch {
            reference: 1,
            target: 4,
            residual: 0.2,
        },
        StarMatch {
            reference: 3,
            target: 5,
            residual: 0.15,
        },
    ];

    let result = RegistrationResult::new(transform, None, matches.clone());

    assert_eq!(result.transform().matrix(), transform.matrix());
    assert!(result.sip_fit().is_none());
    assert_eq!(result.matched_stars(), matches);
    assert_eq!(result.num_inliers(), 3);

    // sqrt((0.1² + 0.2² + 0.15²) / 3) = sqrt(0.0725 / 3).
    let expected_rms = (0.0725_f64 / 3.0).sqrt();
    assert_eq!(result.rms_error().to_bits(), expected_rms.to_bits());
    assert_eq!(result.max_error().to_bits(), 0.2_f64.to_bits());
    assert_eq!(result.quality_score(), 0.0);
}

#[test]
fn empty_result_has_zero_diagnostics() {
    let result = RegistrationResult::new(Transform::identity(), None, Vec::new());

    assert!(result.matched_stars().is_empty());
    assert_eq!(result.num_inliers(), 0);
    assert_eq!(result.rms_error(), 0.0);
    assert_eq!(result.max_error(), 0.0);
    assert_eq!(result.quality_score(), 0.0);
}

#[test]
fn quality_score_uses_error_and_saturating_inlier_factors() {
    let four_residuals = [0.1, 0.2, 0.15, 0.05];
    let four = RegistrationResult::new(
        Transform::identity(),
        None,
        identity_matches(&four_residuals),
    );

    // RMS = sqrt((0.01 + 0.04 + 0.0225 + 0.0025) / 4).
    let expected_rms = (0.075_f64 / 4.0).sqrt();
    let expected_quality = (-expected_rms / 2.0).exp() * (4.0 / 20.0);
    assert!((four.rms_error() - expected_rms).abs() < f64::EPSILON);
    assert_eq!(four.max_error().to_bits(), 0.2_f64.to_bits());
    assert!((four.quality_score() - expected_quality).abs() < f64::EPSILON);

    let twenty_five =
        RegistrationResult::new(Transform::identity(), None, identity_matches(&[0.1; 25]));
    // The inlier factor saturates at 20, so quality = exp(-0.1 / 2).
    let saturated_quality = (-0.05_f64).exp();
    assert!((twenty_five.rms_error() - 0.1).abs() < f64::EPSILON);
    assert!((twenty_five.quality_score() - saturated_quality).abs() < f64::EPSILON);
}

#[test]
fn elapsed_time_and_warp_transform_preserve_result_state() {
    let transform = Transform::translation(DVec2::new(5.0, -3.0));
    let result = RegistrationResult::new(
        transform,
        None,
        identity_matches(&[0.1, 0.2, 0.15, 0.05, 0.12]),
    )
    .with_elapsed(42.5);

    assert_eq!(result.elapsed_ms(), 42.5);

    let warp = result.warp_transform();
    assert!(!warp.has_sip());
    assert_eq!(warp.apply(DVec2::new(10.0, 20.0)), DVec2::new(15.0, 17.0));
}

#[test]
fn registration_error_messages_include_context() {
    let cases = [
        (
            RegistrationError::InsufficientStars {
                found: 5,
                required: 10,
            },
            "Insufficient stars detected: found 5, need 10",
        ),
        (
            RegistrationError::NoMatchingPatterns,
            "No matching star patterns found between images",
        ),
        (
            RegistrationError::RansacFailed {
                reason: RansacFailureReason::NoInliersFound,
                iterations: 1000,
                best_inlier_count: 0,
            },
            "RANSAC failed: no inliers found (iterations: 1000, best inlier count: 0)",
        ),
        (
            RegistrationError::AccuracyTooLow {
                rms_error: 5.123,
                max_allowed: 2.0,
            },
            "Registration accuracy too low: 5.123 pixels (max: 2.000)",
        ),
        (
            RegistrationError::StarDetection("threshold too high".to_string()),
            "Star detection failed: threshold too high",
        ),
        (
            RegistrationError::SipPointCountMismatch {
                reference: 12,
                target: 10,
            },
            "SIP point count mismatch: 12 reference points, 10 target points",
        ),
        (
            RegistrationError::InsufficientSipPoints {
                found: 8,
                required: 9,
            },
            "Insufficient points for SIP fit: found 8, need 9",
        ),
        (
            RegistrationError::SingularSipSystem,
            "SIP fit failed: singular polynomial system",
        ),
    ];

    for (error, expected) in cases {
        assert_eq!(error.to_string(), expected);
    }
}

#[test]
fn ransac_failure_reason_messages_are_specific() {
    let cases = [
        (RansacFailureReason::NoInliersFound, "no inliers found"),
        (
            RansacFailureReason::DegeneratePointSet,
            "degenerate point set",
        ),
        (RansacFailureReason::SingularMatrix, "singular matrix"),
        (
            RansacFailureReason::InsufficientInliers,
            "insufficient inliers",
        ),
    ];

    for (reason, expected) in cases {
        assert_eq!(reason.to_string(), expected);
    }
}
