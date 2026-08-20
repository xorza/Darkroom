use glam::Vec2;

use crate::float_ext::FloatExt;

#[test]
fn tolerance_tracks_magnitude() {
    // 1e9 lies in [2^29, 2^30), so one ULP there is 2^29 * 2^-23 = 64 and
    // every offset below is exact in f32. The tolerance is
    // 8 * 2^-23 * 1e9 = 953.674, which these three bracket to within one ULP.
    assert!(1e9_f32.approximately_eq(1e9 + 64.0), "1 ULP apart");
    assert!(
        1e9_f32.approximately_eq(1e9 + 896.0),
        "14 ULP is under 953.674"
    );
    assert!(
        !1e9_f32.approximately_eq(1e9 + 960.0),
        "15 ULP is over 953.674"
    );
    assert!(!1e9_f32.approximately_eq(1.001e9), "a real difference");
    // An absolute tolerance of 1e-6 could not see any of that: at 1e9 an f32
    // cannot represent a difference below 64 in the first place, so the test
    // collapsed into `==`. At unit magnitude that same tolerance is sane —
    // which is the contradiction one fixed number cannot resolve.
    assert!(!1.0_f32.approximately_eq(1.0 + 64.0));
}

#[test]
fn small_magnitudes_are_not_all_equal() {
    // A thousandfold difference. The tolerance at 1e-9 is 2^-20 * 1e-9
    // ≈ 9.5e-16, against a difference of 9.99e-10.
    assert!(!1e-9_f32.approximately_eq(1e-12));
    assert!(!1e-9_f64.approximately_eq(1e-12));
    // Same-value still holds down there: 4 ULP above 1e-9 is half the
    // tolerance away from it.
    let four_ulp = 1e-9_f32 * (1.0 + 4.0 * f32::EPSILON);
    assert!(1e-9_f32.approximately_eq(four_ulp));
}

#[test]
fn zero_is_a_value_not_a_neighbourhood() {
    assert!(0.0_f32.approximately_eq(0.0));
    assert!(0.0_f32.approximately_eq(-0.0), "IEEE 754 calls these equal");
    // A relative tolerance around zero is zero. Anything that did not cancel
    // exactly is a different value.
    assert!(!0.0_f32.approximately_eq(f32::MIN_POSITIVE));
    assert!(!0.0_f32.approximately_eq(1e-30));
    assert!(!0.0_f64.approximately_eq(1e-300));
}

#[test]
fn each_width_uses_its_own_epsilon() {
    // An f32-scaled tolerance is ~1e-6; f64's own is 8 * 2^-52 ≈ 1.8e-15, so
    // a gap of 1e-9 is six orders of magnitude too wide to pass.
    assert!(!1.0_f64.approximately_eq(1.0 + 1e-9));
    // Stated in ULP the two widths agree, which is the property the shared
    // impl exists to guarantee.
    for ulps in [1u8, 4, 8] {
        assert!(
            1.0_f32.approximately_eq(1.0 + f32::from(ulps) * f32::EPSILON),
            "{ulps} ULP is within the f32 tolerance of 8"
        );
        assert!(
            1.0_f64.approximately_eq(1.0 + f64::from(ulps) * f64::EPSILON),
            "{ulps} ULP is within the f64 tolerance of 8"
        );
    }
    for ulps in [16u8, 64] {
        assert!(
            !1.0_f32.approximately_eq(1.0 + f32::from(ulps) * f32::EPSILON),
            "{ulps} ULP is past the f32 tolerance of 8"
        );
        assert!(
            !1.0_f64.approximately_eq(1.0 + f64::from(ulps) * f64::EPSILON),
            "{ulps} ULP is past the f64 tolerance of 8"
        );
    }
}

#[test]
fn nan_and_infinity() {
    assert!(!f32::NAN.approximately_eq(f32::NAN));
    assert!(!f32::NAN.approximately_eq(0.0));
    assert!(!0.0_f32.approximately_eq(f32::NAN));
    assert!(!f64::NAN.approximately_eq(f64::NAN));
    // Only the exact-equality branch may answer for infinities: the
    // magnitude-scaled tolerance is itself infinite there, and `INF <= INF`
    // would otherwise call an infinity equal to every finite value.
    assert!(f32::INFINITY.approximately_eq(f32::INFINITY));
    assert!(f32::NEG_INFINITY.approximately_eq(f32::NEG_INFINITY));
    assert!(!f32::INFINITY.approximately_eq(f32::NEG_INFINITY));
    assert!(!f32::INFINITY.approximately_eq(f32::MAX));
    assert!(!f32::INFINITY.approximately_eq(1.0));
    assert!(!1.0_f32.approximately_eq(f32::INFINITY));
    // Finite operands whose difference overflows still answer finitely.
    assert!(!f32::MAX.approximately_eq(-f32::MAX));
}

#[test]
fn signs_and_symmetry() {
    assert!((-5.0_f32).approximately_eq(-5.0));
    assert!(!(-5.0_f32).approximately_eq(-5.01));
    assert!(!(-1.0_f32).approximately_eq(1.0));
    // `max(|a|, |b|)` is symmetric, so the verdict has to be, on both sides
    // of the threshold.
    let near = 1.0 + 4.0 * f32::EPSILON;
    let far = 1.0 + 64.0 * f32::EPSILON;
    assert_eq!(1.0_f32.approximately_eq(near), near.approximately_eq(1.0));
    assert_eq!(1.0_f32.approximately_eq(far), far.approximately_eq(1.0));
    assert!(1.0_f32.approximately_eq(near) && !1.0_f32.approximately_eq(far));
}

#[test]
fn vec2_judges_each_axis_at_its_own_scale() {
    // Tolerances here are 2^-20 * 4096 = 0.0039 in x and 2^-20 * 0.5 = 4.8e-7
    // in y — four orders of magnitude apart on the same value.
    let pan = Vec2::new(4096.0, 0.5);
    assert!(pan.approximately_eq(Vec2::new(4096.0 + 0.003, 0.5 + 3.0e-7)));
    // x still inside its tolerance, y past its own: the pair is not equal.
    assert!(!pan.approximately_eq(Vec2::new(4096.0 + 0.003, 0.5 + 1.0e-5)));
    // …and the converse, which is what one shared tolerance could not do.
    assert!(!pan.approximately_eq(Vec2::new(4096.0 + 1.0, 0.5)));
    assert!(!Vec2::new(f32::NAN, 0.0).approximately_eq(Vec2::new(f32::NAN, 0.0)));
}
