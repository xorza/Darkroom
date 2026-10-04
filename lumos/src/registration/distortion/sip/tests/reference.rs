use super::*;
use crate::registration::point_normalization::centroid;

/// SIP has no constant or linear terms — those belong to the transform — so a radial field is a
/// SIP polynomial only about its own centre. Points in one quadrant of a field centred at
/// (512, 384): fitted about that centre, exact; about the points' centroid, the field's shift of
/// origin leaves linear terms the polynomial cannot hold.
#[test]
fn a_radial_field_is_exact_only_about_its_centre() {
    let field = RadialField::new(DVec2::new(512.0, 384.0), 1e-7);
    let mut reference = Vec::new();
    for y in (100..=350).step_by(50) {
        for x in (100..=450).step_by(50) {
            reference.push(DVec2::new(f64::from(x), f64::from(y)));
        }
    }
    let target: Vec<DVec2> = reference.iter().map(|&p| field.image(p)).collect();
    let rms_about = |origin| {
        let sip =
            SipPolynomial::fitted_under(&Transform::identity(), &reference, &target, 3, origin);
        rms(&sip.corrected_residuals(&reference, &target, &Transform::identity()))
    };
    let centred = rms_about(field.centre);
    let centroid = rms_about(centroid(&reference));
    assert!(centred <= EXACT_FIT_PX, "{centred:e}");
    assert!(centroid > 1e3 * EXACT_FIT_PX, "{centroid:e}");
}

/// A narrow strip — x over 1000 px, y over 100 — makes the v-dependent monomials tiny beside the
/// u-dependent ones: the order-5 design's condition number is 5.1e5, and its normal equations would
/// square it. The SVD solve on the design keeps it, so the exact cubic field is recovered inside the
/// strip to `κ·ε` of the largest correction, 25 px: 3e-9 px, and 1e-8 holds it with the
/// evaluation's own rounding.
#[test]
fn a_narrow_strip_is_fitted_to_rounding() {
    let field = barrel();
    let center = field.centre;
    let mut ref_points = Vec::new();
    for y in (450..=550).step_by(10) {
        for x in (0..=1000).step_by(20) {
            ref_points.push(DVec2::new(f64::from(x), f64::from(y)));
        }
    }
    let target_points: Vec<DVec2> = ref_points.iter().map(|&p| field.image(p)).collect();
    // 11 y-values × 51 x-values = 561 points; order 5 needs 3 × 18 = 54.

    let transform = Transform::identity();
    let sip = SipPolynomial::fitted_under(&transform, &ref_points, &target_points, 5, center);

    for x_val in (0..=1000).step_by(100) {
        let p = DVec2::new(f64::from(x_val), 500.0);
        let error = (transform.apply(sip.correct(p)) - field.image(p)).length();
        assert!(error <= 1e-8, "strip fit error at x={x_val}: {error:e} px");
    }
}

/// Points on one line through the reference point have `v = 0`, so every term in `v` is a zero
/// column and the system has no unique solution: the fit is refused, not solved by a pseudo-inverse
/// that would pick one of infinitely many.
#[test]
fn a_rank_deficient_layout_is_refused() {
    let center = DVec2::new(500.0, 500.0);
    let reference: Vec<DVec2> = (0..=100)
        .map(|i| DVec2::new(10.0 * f64::from(i), 500.0))
        .collect();
    let target: Vec<DVec2> = reference
        .iter()
        .map(|&p| p + DVec2::new(0.5, 0.0))
        .collect();
    let weights = vec![1.0; reference.len()];
    let pairs = SipPairs {
        reference: &reference,
        target: &target,
        weights: &weights,
    };
    assert!(SipPolynomial::fit_under(&Transform::identity(), pairs, 3, center).is_none());
    assert!(SipPolynomial::fit_with(TransformType::Affine, pairs, 3, center).is_none());
}
