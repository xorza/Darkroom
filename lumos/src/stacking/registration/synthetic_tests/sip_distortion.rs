//! SIP distortion-recovery through the full `register()` path.
//!
//! `warping.rs` fits a SIP polynomial from explicit matched point pairs; this drives a known
//! radial (barrel) optical distortion through `register()` end-to-end and verifies the SIP fit
//! recovers it — the residuals collapse versus a linear-only registration of the same field.

use crate::stacking::registration::distortion::sip::SipConfig;
use crate::stacking::registration::synthetic_tests::helpers;
use crate::stacking::registration::transform::Transform;
use crate::stacking::registration::{Config, register};
use crate::testing::synthetic::transforms::{generate_random_positions, positions_to_stars};
use glam::DVec2;

/// Apply a radial barrel distortion `p' = p + k·r²·(p − center)` about `center`.
fn barrel(p: DVec2, center: DVec2, k: f64) -> DVec2 {
    let d = p - center;
    p + d * (k * d.length_squared())
}

#[test]
fn register_with_sip_recovers_barrel_distortion() {
    let center = DVec2::new(512.0, 512.0);
    let ref_pos = generate_random_positions(120, 1024.0, 1024.0, 42);

    // Target = a small linear shift composed with a cubic barrel distortion (~3–4 px at corners).
    let base = Transform::translation(DVec2::new(7.0, -4.0));
    let k = 1e-8;
    let target_pos: Vec<DVec2> = ref_pos
        .iter()
        .map(|&p| base.apply(barrel(p, center, k)))
        .collect();

    let ref_stars = positions_to_stars(&ref_pos, 3.0);
    let target_stars = positions_to_stars(&target_pos, 3.0);

    let base_config = Config {
        matching: helpers::matching_config(20, 10),
        max_rms_error: 10.0, // high gate so both registrations return (we compare their RMS)
        ..Config::default()
    };
    let no_sip = register(
        &ref_stars,
        &target_stars,
        &Config {
            sip: None,
            ..base_config.clone()
        },
    )
    .expect("linear registration should succeed");
    let with_sip = register(
        &ref_stars,
        &target_stars,
        &Config {
            sip: Some(SipConfig {
                order: 3,
                reference_point: Some(center),
                ..Default::default()
            }),
            ..base_config
        },
    )
    .expect("SIP registration should succeed");

    assert!(
        with_sip.sip_fit().is_some(),
        "a SIP polynomial should have been fitted"
    );
    // A linear transform cannot absorb the radial barrel → a visible residual remains.
    assert!(
        no_sip.rms_error() > 0.3,
        "linear-only should leave a barrel residual, got RMS {:.3}",
        no_sip.rms_error()
    );
    // The order-3 SIP captures the cubic distortion → residuals collapse.
    assert!(
        with_sip.rms_error() < no_sip.rms_error() * 0.5,
        "SIP should at least halve the residual: {:.3} vs {:.3}",
        with_sip.rms_error(),
        no_sip.rms_error()
    );
    assert!(
        with_sip.rms_error() < 0.2,
        "SIP-corrected RMS {:.3} should be small",
        with_sip.rms_error()
    );
}
