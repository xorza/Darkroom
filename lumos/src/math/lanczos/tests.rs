use std::f64::consts::PI;

use crate::internals::assertions::assert_close;
use crate::math::lanczos::kernel;

/// `sinc` at a nonzero integer reads `sin` of the rounded f32 product `n·π`, which is off a true
/// zero by that rounding: at most half an ulp of `4π` (2⁻²¹ ≈ 4.8e-7) plus `n` times π's own
/// rounding (8.7e-8), over a divisor of at least π. 1e-6 covers it.
const TOL: f32 = 1e-6;

#[test]
fn lanczos_kernel_at_zero() {
    // L(0, a) = 1 by definition (the limit of sinc(x)·sinc(x/a) as x → 0), returned as is.
    for a in [2.0, 3.0, 4.0] {
        assert_eq!(kernel(0.0, a), 1.0);
    }
}

#[test]
fn lanczos_kernel_at_integers() {
    // sinc(n) = sin(n*pi) / (n*pi) = 0 for all nonzero integers
    // So L(n, a) = 0 for integer n != 0
    for a in [2.0, 3.0, 4.0] {
        for n in 1..(a as i32) {
            assert_close!(kernel(n as f32, a), 0.0, TOL, "L({n}, {a})");
            assert_close!(kernel(-(n as f32), a), 0.0, TOL, "L(-{n}, {a})");
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
/// The two differ, so `a` reaches the window. The f32 kernel rounds π, its two products, two
/// quotients and two `sin`s by half an ulp or so each: within 8ε of the f64 values.
#[test]
fn lanczos_kernel_at_half() {
    for (a, expected) in [(3.0, 6.0 / (PI * PI)), (2.0, 4.0 * 2f64.sqrt() / (PI * PI))] {
        assert_close!(kernel(0.5, a), expected, 8.0 * f32::EPSILON, "L(0.5, {a})");
    }
}

/// `L(−x) = L(x)` exactly: `π·(−x)` is the negated product, and `sin` is odd bit for bit.
#[test]
fn lanczos_kernel_symmetry() {
    for &x in &[0.1, 0.5, 1.0, 1.5, 2.5] {
        assert_eq!(kernel(-x, 3.0), kernel(x, 3.0), "L(±{x})");
    }
}
