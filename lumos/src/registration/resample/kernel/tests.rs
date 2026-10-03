use std::f64::consts::PI;

use crate::internals::prelude::*;
use crate::registration::resample::kernel::{self, LANCZOS_LUT_RESOLUTION, LanczosOrder};
use crate::registration::resample::source_position::SourcePosition;

/// `sinc(πx)·sinc(πx/a)` in f64, from its definition.
fn lanczos_f64(x: f64, a: f64) -> f64 {
    if x == 0.0 {
        return 1.0;
    }
    let sinc = |t: f64| t.sin() / t;
    sinc(PI * x) * sinc(PI * x / a)
}

/// A table read lands on the nearest entry, so it is the kernel within half a step of the probe.
///
/// The probes sit halfway between entries and one ulp either side, where the rounding of the
/// index is decided and the quantization error is largest: there the read is off the kernel by at
/// most `max|L′|·(½·step + ulp)`. `sinc′(t)` peaks at 0.4362 (t ≈ 2.08), so `½` bounds it and
/// `|L′| ≤ π·½·(1 + 1/a)` by the product rule. Each entry is the f32 kernel, which is off the true
/// one by 1e-6 at most (see `math::lanczos`'s tests). On the grid itself the read is that entry.
/// The worst error seen has to reach a quarter of the bound, or the probes missed the midpoints.
#[test]
fn a_lanczos_table_read_is_within_half_a_step_of_the_kernel() {
    let step = 1.0 / LANCZOS_LUT_RESOLUTION as f64;
    for order in [LanczosOrder::Two, LanczosOrder::Three, LanczosOrder::Four] {
        let lut = order.lut();
        let a = order.a() as f64;
        for k in 0..=order.a() * LANCZOS_LUT_RESOLUTION {
            let x = (k as f64 * step) as f32;
            assert_eq!(lut.lookup_positive(x), lut.values[k], "a = {a}, entry {k}");
        }

        let slope = PI * 0.5 * (1.0 + 1.0 / a);
        let ulp = f64::from(f32::EPSILON) * a;
        let bound = slope * (0.5 * step + ulp) + 1e-6;
        let mut worst = 0.0f64;
        for k in (0..order.a() * LANCZOS_LUT_RESOLUTION).step_by(7) {
            let midpoint = ((k as f64 + 0.5) * step) as f32;
            for x in [midpoint.next_down(), midpoint, midpoint.next_up()] {
                let error =
                    (f64::from(lut.lookup_positive(x)) - lanczos_f64(f64::from(x), a)).abs();
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

/// Catmull-Rom (`A = −½`) by hand, every value dyadic and so exact in f32:
/// inner branch `(1.5|x| − 2.5)·x² + 1`, outer `((−0.5|x| + 2.5)·|x| − 4)·|x| + 2`.
/// `K(0.25)` = (0.375 − 2.5)·0.0625 + 1 = 0.8671875; `K(0.5)` = (0.75 − 2.5)·0.25 + 1 = 0.5625;
/// `K(0.75)` = (1.125 − 2.5)·0.5625 + 1 = 0.2265625; `K(1)` = 0 from both branches;
/// `K(1.25)` = (1.875·1.25 − 4)·1.25 + 2 = −0.0703125; `K(1.5)` = (1.75·1.5 − 4)·1.5 + 2 = −0.0625;
/// `K(1.75)` = (1.625·1.75 − 4)·1.75 + 2 = −0.0234375; `K(2)` = 0, and nothing beyond.
///
/// Both branches have slope `−½` at 1 (`4.5x² − 5x` and `−1.5x² + 5x − 4`), so across `1 ± 1e-4`
/// the kernel falls by 1e-4; a jump between the branches would show on top of that. The f32
/// evaluation of terms near 2.5 that cancel to 5e-5 rounds to a few ulps of 2.5, under 1e-6.
#[test]
fn bicubic_kernel_hand_values() {
    for (x, expected) in [
        (0.0, 1.0),
        (0.25, 0.867_187_5),
        (0.5, 0.5625),
        (0.75, 0.226_562_5),
        (1.0, 0.0),
        (1.25, -0.070_312_5),
        (1.5, -0.0625),
        (1.75, -0.023_437_5),
        (2.0, 0.0),
        (2.5, 0.0),
    ] {
        assert_eq!(kernel::bicubic_kernel(x), expected, "K({x})");
        assert_eq!(kernel::bicubic_kernel(-x), expected, "K(-{x})");
    }
    let fall = kernel::bicubic_kernel(1.0 - 1e-4) - kernel::bicubic_kernel(1.0 + 1e-4);
    assert!((fall - 1e-4).abs() < 1e-6, "{fall}");
    assert_eq!(
        kernel::bicubic_weights(0.25),
        [-0.070_312_5, 0.867_187_5, 0.226_562_5, -0.023_437_5]
    );
}

/// The nearest pixel, a half rounding up: on `[[10, 20], [30, 40]]`, (0.4, 0.4) is pixel (0, 0),
/// (1.4, 0.4) is (1, 0), (0.4, 1.4) is (0, 1), and (0.5, 0) rounds up to (1, 0). In the rim past
/// the last centre it holds to the edge pixel: (1.45, −0.45) is (1, 0).
#[test]
fn nearest_rounds_a_half_up() {
    let input = Buffer2::new(2, 2, vec![10.0, 20.0, 30.0, 40.0]);
    let size = Size2us::new(2, 2);
    for (x, y, expected) in [
        (0.4, 0.4, 10.0),
        (1.4, 0.4, 20.0),
        (0.4, 1.4, 30.0),
        (1.4, 1.4, 40.0),
        (0.5, 0.0, 20.0),
        (1.45, -0.45, 20.0),
    ] {
        let position = SourcePosition::within(DVec2::new(x, y), size).unwrap();
        assert_eq!(
            kernel::nearest_sample(&input, position),
            expected,
            "({x}, {y})"
        );
    }
}

/// A 3×3 ramp 0..8, sampled between pixel centres and blended by hand:
/// at (0.5, 0.5) top 0 → 1 is 0.5, bottom 3 → 4 is 3.5, and halfway between them 2;
/// at (1.5, 0.5) the same over 1, 2, 4, 5 gives 3; at (0.25, 0.75) top 0.25, bottom 3.25,
/// three quarters of the way 2.5; in the rim at (2.25, 2.4) both axes hold to the last centre, 8.
/// A constant 7 reads back 7 anywhere: the blend adds `f·0`. All exact in f32.
#[test]
fn bilinear_sample_hand_computed() {
    let input = Buffer2::new(3, 3, (0..9).map(|i| i as f32).collect());
    let constant = Buffer2::new_filled(3, 3, 7.0);
    let size = Size2us::new(3, 3);
    for (x, y, expected) in [
        (1.0, 1.0, 4.0),
        (0.5, 0.5, 2.0),
        (1.5, 0.5, 3.0),
        (0.25, 0.75, 2.5),
        (2.25, 2.4, 8.0),
    ] {
        let position = SourcePosition::within(DVec2::new(x, y), size).unwrap();
        assert_eq!(
            kernel::bilinear_sample(&input, position),
            expected,
            "({x}, {y})"
        );
        assert_eq!(
            kernel::bilinear_sample(&constant, position),
            7.0,
            "({x}, {y})"
        );
    }
}

/// Bilinear reproduces a plane and Catmull-Rom a quadratic — any `xⁱyʲ` with `i, j ≤ 2`, since its
/// 1-D weights reproduce `1, x, x²` — at every interior fraction, to rounding.
///
/// The rounding: the pixels are the f32 field (½ ulp each), and each of the up to 16 products and
/// sums rounds once against the running magnitude, at most `Σ|w|·max|v|`; Catmull-Rom's
/// `Σ|w|` peaks at `f = ½`, `(2·0.5625 + 2·0.0625)² = 1.5625`. So `34·ε·1.5625·max|v|`.
///
/// Bilinear does not reproduce the quadratic, which is what makes the second claim a test: at a
/// cell's centre it averages the four corners, which overshoots `x²` by `¼` and reproduces `xy`
/// exactly, so on `0.02x² − 0.03xy + 0.05y² + …` it is high by `(0.02 + 0.05)/4 = 0.0175`.
#[test]
fn bilinear_and_bicubic_reproduce_their_polynomials() {
    let size = Size2us::new(12, 10);
    let plane = |x: f64, y: f64| 0.25 * x - 0.5 * y + 3.0;
    let quadratic =
        |x: f64, y: f64| 0.02 * x * x - 0.03 * x * y + 0.05 * y * y + 0.1 * x - 0.2 * y + 1.0;
    let image = |field: &dyn Fn(f64, f64) -> f64| {
        Buffer2::new(
            size.width,
            size.height,
            (0..size.pixel_count())
                .map(|i| field((i % size.width) as f64, (i / size.width) as f64) as f32)
                .collect(),
        )
    };
    let plane_image = image(&plane);
    let quadratic_image = image(&quadratic);
    let largest = |image: &Buffer2<f32>| image.pixels().iter().fold(0.0f32, |m, v| m.max(v.abs()));
    let plane_bound = 34.0 * f32::EPSILON * largest(&plane_image);
    let quadratic_bound = 34.0 * f32::EPSILON * 1.5625 * largest(&quadratic_image);

    for (fx, fy) in [(0.125, 0.375), (0.5, 0.5), (0.8, 0.1)] {
        for cell_y in 1..size.height - 2 {
            for cell_x in 1..size.width - 2 {
                let p = DVec2::new(cell_x as f64 + fx, cell_y as f64 + fy);
                let position = SourcePosition::within(p, size).unwrap();
                // The kernels sample the narrowed fraction, so the truth is taken there.
                let (x, y) = (
                    f64::from(position.cell_x) + f64::from(position.fx),
                    f64::from(position.cell_y) + f64::from(position.fy),
                );
                let linear = f64::from(kernel::bilinear_sample(&plane_image, position));
                assert!(
                    (linear - plane(x, y)).abs() <= f64::from(plane_bound),
                    "bilinear at {p:?}: {linear} against {}",
                    plane(x, y)
                );
                let cubic = f64::from(kernel::bicubic_sample(&quadratic_image, position));
                assert!(
                    (cubic - quadratic(x, y)).abs() <= f64::from(quadratic_bound),
                    "bicubic at {p:?}: {cubic} against {}",
                    quadratic(x, y)
                );
            }
        }
    }

    let centre = SourcePosition::within(DVec2::new(5.5, 4.5), size).unwrap();
    let overshoot =
        f64::from(kernel::bilinear_sample(&quadratic_image, centre)) - quadratic(5.5, 4.5);
    assert!(
        (overshoot - 0.0175).abs() <= f64::from(quadratic_bound),
        "bilinear on the quadratic is off by {overshoot}"
    );
}
