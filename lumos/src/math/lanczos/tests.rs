use std::f64::consts::PI;

use crate::math::lanczos::kernel;
use crate::math::lanczos::lanczos_lut::{LANCZOS_LUT_RESOLUTION, LanczosOrder};

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

/// `sinc(πx)·sinc(πx/a)` in f64, from its definition.
fn lanczos_f64(x: f64, a: f64) -> f64 {
    if x == 0.0 {
        return 1.0;
    }
    let sinc = |t: f64| t.sin() / t;
    sinc(PI * x) * sinc(PI * x / a)
}

/// A table read is the line between the entries either side, so it is the kernel within the linear
/// interpolation's error and two roundings.
///
/// The interpolation is off by at most `max|L″|·h²/8`, `h` the step. With `|sinc| ≤ 1`,
/// `|sinc′| ≤ 0.4362` and `|sinc″| ≤ ⅓` (its peak, at 0), the product rule bounds
/// `|L″| ≤ π²·(⅓ + 2·0.4362²/a + 1/(3a²))`: 5.99 for a = 2. Each entry is the kernel rounded once
/// to f32, off by at most half a step at 1, `2⁻²⁵`, and the read's fused multiply-add rounds once
/// more. The probes sit halfway between entries, where the interpolation error peaks, and one ulp
/// either side. On the grid itself the read is that entry. The worst error seen has to reach a
/// quarter of the bound, or the probes missed the curvature.
#[test]
fn a_lanczos_table_read_is_the_kernel_within_the_interpolation_error() {
    let step = 1.0 / LANCZOS_LUT_RESOLUTION as f64;
    let rounding = 2.0f64.powi(-25);
    for order in [LanczosOrder::Two, LanczosOrder::Three, LanczosOrder::Four] {
        let lut = order.lut();
        let a = order.a() as f64;
        for k in 0..=order.a() * LANCZOS_LUT_RESOLUTION {
            let x = (k as f64 * step) as f32;
            assert_eq!(
                lut.at(x * LANCZOS_LUT_RESOLUTION as f32),
                lut.values[k],
                "a = {a}, entry {k}"
            );
        }

        let curvature = PI * PI * (1.0 / 3.0 + 2.0 * 0.4362 * 0.4362 / a + 1.0 / (3.0 * a * a));
        let bound = curvature * step * step / 8.0 + 2.0 * rounding;
        let mut worst = 0.0f64;
        for k in (0..order.a() * LANCZOS_LUT_RESOLUTION).step_by(7) {
            let midpoint = ((k as f64 + 0.5) * step) as f32;
            for x in [midpoint.next_down(), midpoint, midpoint.next_up()] {
                let error = (f64::from(lut.at(x * LANCZOS_LUT_RESOLUTION as f32))
                    - lanczos_f64(f64::from(x), a))
                .abs();
                assert!(
                    error <= bound,
                    "a = {a}, x = {x}: off by {error}, bound {bound}"
                );
                worst = worst.max(error);
            }
        }
        assert!(
            worst > bound / 4.0,
            "a = {a}: worst {worst} against bound {bound}"
        );
    }
}
