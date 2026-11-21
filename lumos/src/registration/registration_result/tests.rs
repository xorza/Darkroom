use glam::DVec2;

use crate::registration::error::{
    RansacFailure, RegistrationCatalog, RegistrationError, StarField,
};
use crate::registration::match_idxs::MatchIdxs;
use crate::registration::registration_result::{RegistrationResult, StarMatch};
use crate::registration::transform::Transform;
use crate::registration::warp_transform::WarpTransform;

/// The result of `transform` fitted to `matches`.
fn result(transform: Transform, matches: Vec<StarMatch>) -> RegistrationResult {
    RegistrationResult::new(WarpTransform::from(transform), matches)
}

fn identity_matches(residuals: &[f64]) -> Vec<StarMatch> {
    residuals
        .iter()
        .enumerate()
        .map(|(idx, &residual)| StarMatch {
            idxs: MatchIdxs {
                reference: idx,
                target: idx,
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
            idxs: MatchIdxs {
                reference: 0,
                target: 2,
            },
            residual: 0.1,
        },
        StarMatch {
            idxs: MatchIdxs {
                reference: 1,
                target: 4,
            },
            residual: 0.2,
        },
        StarMatch {
            idxs: MatchIdxs {
                reference: 3,
                target: 5,
            },
            residual: 0.15,
        },
    ];

    let result = result(transform, matches.clone());

    assert_eq!(result.warp().linear().matrix(), transform.matrix());
    assert!(result.warp().sip().is_none());
    assert_eq!(result.matches(), matches);
    assert_eq!(result.matches().len(), 3);

    // sqrt((0.1² + 0.2² + 0.15²) / 3) = sqrt(0.0725 / 3).
    let expected_rms = (0.0725_f64 / 3.0).sqrt();
    assert_eq!(result.rms_error_px().to_bits(), expected_rms.to_bits());
}

/// The result hands out the warp it was built from, borrowed or taken whole.
#[test]
fn the_result_hands_out_its_warp() {
    let transform = Transform::translation(DVec2::new(5.0, -3.0));
    let result = result(transform, identity_matches(&[0.1, 0.2, 0.15, 0.05, 0.12]));
    let warp = result.warp();
    assert!(warp.sip().is_none());
    assert_eq!(warp.apply(DVec2::new(10.0, 20.0)), DVec2::new(15.0, 17.0));
    let warp = result.into_warp();
    assert_eq!(warp.apply(DVec2::new(10.0, 20.0)), DVec2::new(15.0, 17.0));
}

#[test]
fn registration_error_messages_include_context() {
    let cases = [
        (
            RegistrationError::TooFewStars {
                catalog: RegistrationCatalog::Target,
                found: 5,
                required: 10,
            },
            "target catalog has 5 stars, and registration needs 10",
        ),
        (
            RegistrationError::NoMatchingPatterns {
                found: 3,
                required: 8,
            },
            "too few matching star patterns between images: found 3, need 8",
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
            RegistrationError::InvalidStarPos {
                catalog: RegistrationCatalog::Reference,
                idx: 3,
                pos: DVec2::new(f64::NAN, 4.5),
            },
            "reference star 3 position must be finite, got (NaN, 4.5)",
        ),
        (
            RegistrationError::InvalidStarField {
                catalog: RegistrationCatalog::Target,
                idx: 7,
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
