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
}
