use glam::DVec2;

use crate::internals::assertions::assert_close;
use crate::registration::transform::Transform;
use crate::registration::warp_transform::WarpTransform;

const EPSILON: f64 = 1e-10;

#[test]
fn warp_transform_new() {
    let t = Transform::translation(DVec2::new(10.0, 5.0));
    let wt = WarpTransform::new(t);
    assert!(!wt.has_sip());

    let p = wt.apply(DVec2::new(1.0, 2.0));
    assert_close!(p.x, 11.0, EPSILON);
    assert_close!(p.y, 7.0, EPSILON);
}

/// A warp with SIP maps `p` to `T(p + c(p))`, bit for bit. The field `k·d·|d|²` about (50, 50) is a
/// cubic, which an order-3 SIP represents exactly, so the fit recovers it to its solve's rounding —
/// well under 1e-9 px on 100 points this size — even at the corner, outside the fitted grid, where
/// `d` = (−50, −50) and the correction is `1e-4·(−50)·5000` = −25 px on each axis.
#[test]
fn warp_transform_with_sip() {
    use crate::internals::synthetic::distortion::{Grid, RadialField, RadialPairs};
    use crate::registration::distortion::sip::SipModel;
    use crate::registration::distortion::sip::sip_order::SipOrder;

    let transform = Transform::similarity(DVec2::new(3.0, -2.0), 0.02, 1.01);
    let field = RadialField {
        transform,
        ..RadialField::new(DVec2::new(50.0, 50.0), 1e-4)
    };
    let RadialPairs { reference, target } = field.pairs(Grid {
        start: 5,
        step: 10,
        extent: 95,
    });
    let sip = SipModel {
        order: SipOrder::Three,
        origin: field.centre,
    }
    .fitted_under(&transform, &reference, &target);

    let wt = WarpTransform::with_sip(transform, sip.clone());
    assert!(wt.has_sip());
    for p in [
        DVec2::ZERO,
        DVec2::new(37.5, 81.25),
        DVec2::new(120.0, -10.0),
    ] {
        assert_eq!(wt.apply(p), transform.apply(sip.correct(p)), "{p:?}");
        let correction = sip.correct(p) - p;
        assert!(
            (correction - field.displacement(p)).length() < 1e-9,
            "{p:?}: correction {correction:?}, field {:?}",
            field.displacement(p)
        );
    }
    assert_eq!(field.displacement(DVec2::ZERO), DVec2::new(-25.0, -25.0));
}

#[test]
fn warp_transform_apply_no_sip_matches_transform() {
    let t = Transform::similarity(DVec2::new(3.0, -2.0), 0.5, 1.1);
    let wt = WarpTransform::new(t);

    for &p in &[
        DVec2::new(0.0, 0.0),
        DVec2::new(100.0, 50.0),
        DVec2::new(-10.0, 200.0),
    ] {
        let from_wt = wt.apply(p);
        let from_t = t.apply(p);
        assert_close!(from_wt.x, from_t.x, EPSILON);
        assert_close!(from_wt.y, from_t.y, EPSILON);
    }
}
