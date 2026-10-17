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
/// the position alone is the same bits; the inverse's Jacobian is the inverse of the forward
/// one; and without SIP it is `T⁻¹` exactly.
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
        let t = warp.apply(r);
        let mapped = inverse.apply(t).unwrap();
        assert_eq!(inverse.position(t), Some(mapped.position));
        assert!(
            (mapped.position - r).length() < NEWTON_TOLERANCE_PX,
            "{r:?} came back as {:?}",
            mapped.position
        );
        // Forward Jacobian by the chain rule, `J_T · (I + ∂c)`; its product with the inverse's
        // is the identity to the rounding of two 2×2 products, 1e-12 at these entry sizes.
        let forward = transform.jacobian(warp.sip.as_ref().unwrap().correct(r))
            * warp.sip.as_ref().unwrap().jacobian(r);
        let identity = forward * mapped.jacobian;
        for (actual, expected) in identity
            .to_cols_array()
            .into_iter()
            .zip([1.0, 0.0, 0.0, 1.0])
        {
            assert!((actual - expected).abs() < 1e-12, "{identity:?}");
        }
    }

    let plain = WarpTransform::new(transform).inverse();
    let t = DVec2::new(33.0, 44.0);
    assert_eq!(
        plain.apply(t).unwrap().position,
        transform.inverse().apply(t)
    );
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
    assert!(inverse.apply(DVec2::new(30.0, 0.0)).is_none());
    let inside = inverse.apply(DVec2::new(10.0, 5.0)).unwrap().position;
    let expected = (1.0 - 0.6f64.sqrt()) / 0.02;
    assert!((inside.x - expected).abs() < 1e-6, "{inside:?}");
    assert!((inside.y - 5.0).abs() < 1e-6, "{inside:?}");
}
