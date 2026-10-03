use std::f32::consts::PI;

use crate::math::lanczos::kernel;

/// `sinc` at a nonzero integer reads `sin` of the rounded f32 product `n·π`, which is off a true
/// zero by that rounding: at most half an ulp of `4π` (2⁻²¹ ≈ 4.8e-7) plus `n` times π's own
/// rounding (8.7e-8), over a divisor of at least π. 1e-6 covers it.
const TOL: f32 = 1e-6;

#[test]
fn lanczos_kernel_at_zero() {
    // L(0, a) = 1.0 by definition (limit of sinc(x) * sinc(x/a) as x -> 0)
    assert!((kernel(0.0, 2.0) - 1.0).abs() < TOL);
    assert!((kernel(0.0, 3.0) - 1.0).abs() < TOL);
    assert!((kernel(0.0, 4.0) - 1.0).abs() < TOL);
}

#[test]
fn lanczos_kernel_at_integers() {
    // sinc(n) = sin(n*pi) / (n*pi) = 0 for all nonzero integers
    // So L(n, a) = 0 for integer n != 0
    for a in [2.0, 3.0, 4.0] {
        for n in 1..(a as i32) {
            let val = kernel(n as f32, a);
            assert!(val.abs() < TOL, "L({n}, {a}) should be 0, got {val}");
            let val_neg = kernel(-(n as f32), a);
            assert!(
                val_neg.abs() < TOL,
                "L({}, {a}) should be 0, got {val_neg}",
                -n
            );
        }
    }
}

#[test]
fn lanczos_kernel_at_boundary() {
    // L(a, a) = 0 by definition (outside support)
    assert_eq!(kernel(3.0, 3.0), 0.0);
    assert_eq!(kernel(-3.0, 3.0), 0.0);
    assert_eq!(kernel(2.0, 2.0), 0.0);
    assert_eq!(kernel(4.0, 4.0), 0.0);
}

#[test]
fn lanczos_kernel_outside_support() {
    assert_eq!(kernel(3.5, 3.0), 0.0);
    assert_eq!(kernel(-4.1, 3.0), 0.0);
    assert_eq!(kernel(100.0, 3.0), 0.0);
}

/// `L(½, a) = sinc(π/2)·sinc(π/(2a))`, with `sinc(π/2) = 2/π`:
/// - a = 3: `sinc(π/6) = ½·6/π`, so `6/π²` = 0.607927;
/// - a = 2: `sinc(π/4) = (√2/2)·4/π`, so `4√2/π²` = 0.573159.
///
/// The two differ, so `a` reaches the window. f32 evaluation is within a few ulps of 0.6.
#[test]
fn lanczos_kernel_at_half() {
    for (a, expected) in [(3.0, 6.0 / (PI * PI)), (2.0, 4.0 * 2f32.sqrt() / (PI * PI))] {
        let actual = kernel(0.5, a);
        assert!(
            (actual - expected).abs() < TOL,
            "L(0.5, {a}) = {expected}, got {actual}"
        );
    }
}

#[test]
fn lanczos_kernel_symmetry() {
    // L(x) = L(-x) for all x
    for &x in &[0.1, 0.5, 1.0, 1.5, 2.5] {
        let pos = kernel(x, 3.0);
        let neg = kernel(-x, 3.0);
        assert!(
            (pos - neg).abs() < TOL,
            "Symmetry broken: L({x}) = {pos}, L(-{x}) = {neg}"
        );
    }
}
