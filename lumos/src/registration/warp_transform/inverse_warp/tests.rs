use glam::DVec2;

use crate::registration::distortion::sip::SipModel;
use crate::registration::distortion::sip::sip_order::SipOrder;
use crate::registration::transform::Transform;
use crate::registration::warp_transform::WarpTransform;
use crate::registration::warp_transform::inverse_warp::NEWTON_TOLERANCE_PX;

/// A SIP fit of `field` over a grid on `[-extent, extent]²` about the origin, on `transform`.
fn sip_on(transform: Transform, extent: i32, field: impl Fn(DVec2) -> DVec2) -> WarpTransform {
    let mut reference = Vec::new();
    for y in (-extent..=extent).step_by(5) {
        for x in (-extent..=extent).step_by(5) {
            reference.push(DVec2::new(f64::from(x), f64::from(y)));
        }
    }
    let target: Vec<DVec2> = reference
        .iter()
        .map(|&r| transform.apply(r + field(r)))
        .collect();
    let sip = SipModel {
        order: SipOrder::Three,
        origin: DVec2::ZERO,
    }
    .fitted_under(&transform, &reference, &target);
    WarpTransform::with_sip(transform, sip)
}

/// Back through the inverse lands where the forward warp started, to the Newton tolerance, and
/// without SIP it is `T⁻¹` exactly.
#[test]
fn the_inverse_undoes_the_forward_warp() {
    let transform = Transform::similarity(DVec2::new(12.0, -7.0), 0.3, 1.05);
    let warp = sip_on(transform, 200, |r| r * 1e-6 * r.length_squared());
    let inverse = warp.inverse();
    for r in [
        DVec2::ZERO,
        DVec2::new(150.0, -90.0),
        DVec2::new(-180.0, 175.0),
    ] {
        let pos = inverse.pos(warp.apply(r)).unwrap();
        assert!(
            (pos - r).length() < NEWTON_TOLERANCE_PX,
            "{r:?} came back as {pos:?}"
        );
    }

    let plain = WarpTransform::new(transform).inverse();
    let t = DVec2::new(33.0, 44.0);
    assert_eq!(plain.pos(t), Some(transform.inverse().apply(t)));
}

/// Past a fold the correction has no inverse, and the iteration says so rather than returning
/// where it stopped. `c(x) = −0.01·x²` along x makes `x + c(x)` peak at 25 (at x = 50), so a
/// target at x = 30 has no preimage; one at x = 10 has `x = (1 − √0.6) / 0.02` ≈ 11.27.
#[test]
fn a_point_past_a_fold_does_not_invert() {
    let warp = sip_on(Transform::identity(), 40, |r| {
        DVec2::new(-0.01 * r.x * r.x, 0.0)
    });
    let inverse = warp.inverse();
    assert!(inverse.pos(DVec2::new(30.0, 0.0)).is_none());
    let inside = inverse.pos(DVec2::new(10.0, 5.0)).unwrap();
    let expected = (1.0 - 0.6f64.sqrt()) / 0.02;
    assert!((inside.x - expected).abs() < 1e-6, "{inside:?}");
    assert!((inside.y - 5.0).abs() < 1e-6, "{inside:?}");
}
