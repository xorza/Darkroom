use std::f64::consts::PI;

use crate::math::lanczos::kernel;

#[test]
fn lanczos_kernel_at_zero() {
    // L(0, a) = 1 by definition (the limit of sinc(x)·sinc(x/a) as x → 0), returned as is.
    for a in [2.0, 3.0, 4.0] {
        assert_eq!(kernel(0.0, a), 1.0);
    }
}

/// `sinc(πn) = 0` at every nonzero integer `n`, so `L(n, a)` is an exact zero: `sin(πt)` reads
/// `t − round(t)`, which is 0 there.
#[test]
fn lanczos_kernel_at_integers() {
    for a in [2.0, 3.0, 4.0] {
        for n in 1..(a as i32) {
            assert_eq!(kernel(n as f32, a), 0.0, "L({n}, {a})");
            assert_eq!(kernel(-(n as f32), a), 0.0, "L(-{n}, {a})");
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
/// The two differ, so `a` reaches the window. The f64 evaluation is a few f64 ulps off these, far
/// inside an f32 rounding step, so the one rounding lands on the f32 nearest each.
#[test]
fn lanczos_kernel_at_half() {
    for (a, expected) in [(3.0, 6.0 / (PI * PI)), (2.0, 4.0 * 2f64.sqrt() / (PI * PI))] {
        assert_eq!(kernel(0.5, a), expected as f32, "L(0.5, {a})");
    }
}

/// `L(−x) = L(x)` exactly: the kernel reads `|x|`.
#[test]
fn lanczos_kernel_symmetry() {
    for &x in &[0.1, 0.5, 1.0, 1.5, 2.5] {
        assert_eq!(kernel(-x, 3.0), kernel(x, 3.0), "L(±{x})");
    }
}
