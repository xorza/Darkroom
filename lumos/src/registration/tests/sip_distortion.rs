//! SIP distortion-recovery through the full `register()` path.
//!
//! `warping.rs` fits a SIP polynomial from explicit matched point pairs; this drives a known
//! radial (barrel) optical distortion through `register()` end-to-end and verifies the SIP fit
//! recovers it — the residuals collapse versus a linear-only registration of the same field.

use crate::internals::prelude::*;
use crate::internals::synthetic::distortion::RadialField;
use crate::internals::synthetic::transforms::{generate_random_positions, positions_to_stars};
use crate::registration::RegistrationConfig;
use crate::registration::distortion::sip::SipConfig;
use crate::registration::register;
use crate::registration::tests::helpers;
use crate::registration::transform::{Transform, TransformType};

/// A shift with a cubic barrel field before it, `d·1e-8·|d|²` about (512, 512) — 3 to 4 px at the
/// corners — on 120 stars. With SIP of order 3 about the field's centre the warp is the field: GRIC
/// takes the fewest parameters that fit it, a rotation and the correction, every pair is matched and
/// the warp lands on every star and every point of the frame to rounding. Without SIP no transform
/// follows the field: the robust fit keeps the pairs it can follow, 108 of the 120 (measured), and
/// still leaves 0.22 px on them.
#[test]
fn register_with_sip_recovers_barrel_distortion() {
    let ref_pos = generate_random_positions(120, 1024.0, 1024.0, 42);
    let field = RadialField {
        transform: Transform::translation(DVec2::new(7.0, -4.0)),
        ..RadialField::new(DVec2::new(512.0, 512.0), 1e-8)
    };
    let target_pos: Vec<DVec2> = ref_pos.iter().map(|&p| field.image(p)).collect();
    let ref_stars = positions_to_stars(&ref_pos, 3.0);
    let target_stars = positions_to_stars(&target_pos, 3.0);

    let base_config = RegistrationConfig {
        matching: helpers::matching_config(20, 10),
        max_rms_error: 10.0,
        ..RegistrationConfig::default()
    };
    let with_sip = register(
        &ref_stars,
        &target_stars,
        &RegistrationConfig {
            sip: Some(SipConfig {
                order: 3,
                reference_point: Some(field.centre),
            }),
            ..base_config.clone()
        },
    )
    .expect("SIP registration should succeed");
    assert_eq!(
        with_sip.transform().transform_type(),
        TransformType::Euclidean
    );
    assert_eq!(with_sip.num_inliers(), 120);
    let warp = with_sip.warp_transform();
    for y in (0..=1024).step_by(128) {
        for x in (0..=1024).step_by(128) {
            let p = DVec2::new(f64::from(x), f64::from(y));
            let miss = warp.apply(p).distance(field.image(p));
            assert!(miss <= 1e-9, "at {p:?}: {miss:e}");
        }
    }

    let no_sip = register(
        &ref_stars,
        &target_stars,
        &RegistrationConfig {
            sip: None,
            ..base_config
        },
    )
    .expect("linear registration should succeed");
    assert!(no_sip.num_inliers() < 120, "{}", no_sip.num_inliers());
    assert!(no_sip.rms_error() > 0.1, "{}", no_sip.rms_error());
}
