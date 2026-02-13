pub trait FloatExt {
    fn approximately_eq(self, other: Self) -> bool;
}

impl FloatExt for f32 {
    fn approximately_eq(self, other: Self) -> bool {
        (self - other).abs() < crate::EPSILON
    }
}

impl FloatExt for f64 {
    fn approximately_eq(self, other: Self) -> bool {
        (self - other).abs() < crate::EPSILON as f64
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn f32_approximately_eq() {
        assert!(1.0_f32.approximately_eq(1.0));
        assert!(0.0_f32.approximately_eq(0.0));
        assert!((0.1_f32 + 0.2_f32).approximately_eq(0.3));
        assert!(!1.0_f32.approximately_eq(1.001));
    }

    #[test]
    fn f64_approximately_eq() {
        assert!(1.0_f64.approximately_eq(1.0));
        assert!(0.0_f64.approximately_eq(0.0));
        assert!((0.1_f64 + 0.2_f64).approximately_eq(0.30000000000000004));
        assert!(!1.0_f64.approximately_eq(1.0001));
    }

    #[test]
    fn f32_nan_is_never_equal() {
        // NaN != NaN per IEEE 754, abs(NaN - NaN) = NaN which is not < EPSILON
        assert!(!f32::NAN.approximately_eq(f32::NAN));
        assert!(!f32::NAN.approximately_eq(0.0));
        assert!(!0.0_f32.approximately_eq(f32::NAN));
    }

    #[test]
    fn f32_infinity_not_approximately_eq_to_finite() {
        // abs(INF - 1.0) = INF which is not < EPSILON
        assert!(!f32::INFINITY.approximately_eq(1.0));
        assert!(!f32::NEG_INFINITY.approximately_eq(-1.0));
        assert!(!1.0_f32.approximately_eq(f32::INFINITY));
    }

    #[test]
    fn f32_negative_values() {
        assert!((-5.0_f32).approximately_eq(-5.0));
        assert!(!(-5.0_f32).approximately_eq(-5.01));
        assert!(!(-1.0_f32).approximately_eq(1.0)); // abs diff = 2.0
    }

    #[test]
    fn f32_at_epsilon_boundary() {
        // EPSILON = 1e-6
        // diff = 0.5e-6 < 1e-6 → true
        assert!(0.0_f32.approximately_eq(0.5e-6));
        // diff = 2e-6 > 1e-6 → false
        assert!(!0.0_f32.approximately_eq(2e-6));
    }

    #[test]
    fn f32_symmetry() {
        let a = 1.0_f32;
        let b = 1.0000005_f32;
        assert_eq!(
            a.approximately_eq(b),
            b.approximately_eq(a),
            "approximately_eq should be symmetric"
        );
    }
}
