use std::ops::RangeInclusive;

use crate::math::fwhm::FWHM_PER_SIGMA;

use super::*;

/// A frame at half the reference's image scale — B, its pixels twice as far apart, each covering
/// four of the reference's — gives every output pixel a quarter of the weight per unit area that an
/// unmagnified frame does, whatever the kernel: every drop deposits its pixel weight in total, and
/// B's drops lie one per 2×2 cells. So its mean weight over one period of its lattice is ¼, against
/// 1 unscaled, at scale 1 and pixfrac 1. The square kernel spreads each drop over the quadrilateral
/// it maps to, `[3, 5]²` for B's pixel (2, 2); the fixed-footprint kernels keep their footprint, as
/// STScI's do, so a cell their drop lands on takes it whole and its neighbours little or nothing.
///
/// At (4, 4), where B's pixel (2, 2) lands, the share B's drops give a cell is so ¼ for the square
/// kernel, 1 for Turbo's 1×1 box, the point kernel and Lanczos, whose taps at the even distances 2
/// and 4 from B's neighbours are zero, and for the Gaussian `((1 + 2g(2))/S)²` with
/// `g(d) = exp(−d²/(2σ²))`, σ = 1/2.3548, and `S = Σ g(d)` over its five taps. Beside A, a uniform
/// 10 unscaled, the cell reads `(10 + b·share) / (1 + share)`.
///
/// A pixel sums at most 25 + 9 deposits of values up to 10; the period mean sums four cells of up to
/// 25 deposits each.
#[test]
fn a_magnified_frame_weighs_less_per_output_pixel() {
    let size = Size2us::new(12, 12);
    let gaussian = |d: f64| (-d * d * FWHM_PER_SIGMA * FWHM_PER_SIGMA / 2.0).exp();
    let s: f64 = (-2..=2).map(|d| gaussian(f64::from(d))).sum();
    let bound = (2.0 * 34.0 + 1.0) * f64::from(f32::EPSILON) * 10.0;
    let magnified = Transform::scale(DVec2::splat(2.0));
    for kernel in DrizzleKernel::ALL {
        let share = match kernel {
            DrizzleKernel::Square => 0.25,
            DrizzleKernel::Gaussian => ((1.0 + 2.0 * gaussian(2.0)) / s).powi(2),
            DrizzleKernel::Turbo | DrizzleKernel::Point | DrizzleKernel::Lanczos => 1.0,
        };
        for (transform, period_mean) in [(magnified, 0.25), (Transform::identity(), 1.0)] {
            let mut alone = accumulator(
                ImageDimensions::new(size, 1),
                kernel_config(kernel, 1.0, 1.0),
            );
            alone.add_image(constant_image(size, 1.0), &transform, None);
            let weights = alone.accumulated_weights();
            let mean = [(4, 4), (5, 4), (4, 5), (5, 5)]
                .iter()
                .map(|&at| f64::from(weights[at]))
                .sum::<f64>()
                / 4.0;
            assert!(
                (mean - period_mean).abs() <= 4.0 * 25.0 * f64::from(f32::EPSILON),
                "{kernel:?} {transform:?}: mean weight {mean}"
            );
        }
        for b in [2.0, 0.0] {
            for (transform, b_weight) in [(magnified, share), (Transform::identity(), 1.0)] {
                let mut acc = accumulator(
                    ImageDimensions::new(size, 1),
                    kernel_config(kernel, 1.0, 1.0),
                );
                acc.add_image(constant_image(size, 10.0), &Transform::identity(), None);
                acc.add_image(constant_image(size, b as f32), &transform, None);
                let actual = f64::from(acc.finalize().image.channel(0)[(4, 4)]);
                let expected = (10.0 + b * b_weight) / (1.0 + b_weight);
                assert!(
                    (actual - expected).abs() <= bound,
                    "{kernel:?} b = {b}, B weight {b_weight}: {actual}, expected {expected}"
                );
            }
        }
    }
}

/// On a linear ramp the reconstruction is the ramp at each output pixel's preimage — a check that
/// sees a dropped or wrong Jacobian term.
///
/// A quarter-pixel shift along x and 0.15 along y, at scale 1 and pixfrac 1: output pixel `o`
/// takes ¾ of input pixel `o` and ¼ of `o − 1` along x, 0.85 and 0.15 along y, so it reads the ramp
/// at `o − (0.25, 0.15)` — for Turbo and the square kernel, whose shares those are.
///
/// Halving the image scale as well — input pixel `i` at `i/2 + ¼` — magnifies by ¼, and still
/// reads the ramp at the preimage `2(o − ¼)` for every kernel. The square kernel's quarter-size
/// quadrilaterals `[i/2, i/2 + ½]` tile each cell with two whole ones; Turbo's unit boxes overlap
/// a cell with shares ¼, ¾, ¾, ¼ at centres `o ∓ ¾`, `o ∓ ¼`; the point kernel puts the pixels at
/// `o ∓ ¼` on it; and the radial kernels' taps are symmetric about `o` on a lattice symmetric about
/// it. A symmetric weighting of a linear function is its value at the centre.
///
/// A pixel sums at most 144 deposits — 12 Lanczos centres per axis in reach at spacing ½ — of
/// values up to 4.5, through lobes summing to 1.7 times the weight, and each Lanczos tap is within
/// 1e-6 of its true value (see `math::lanczos`'s tests), 12 per axis, which moves a window whose
/// values differ from the centre by at most 3.5·0.15 per axis.
#[test]
fn a_linear_ramp_reads_back_at_each_preimage() {
    let size = Size2us::new(24, 24);
    let ramp = |x: f64, y: f64| 1.0 + 0.1 * x + 0.05 * y;
    let image = gray_image(
        size,
        (0..size.pixel_count())
            .map(|i| ramp((i % 24) as f64, (i / 24) as f64) as f32)
            .collect(),
    );
    let bound =
        (2.0 * 144.0 + 1.0) * f64::from(f32::EPSILON) * 4.5 * 1.7 + 2.0 * 12.0 * 1e-6 * 3.5 * 0.15;

    let shifted = Transform::translation(DVec2::new(0.25, 0.15));
    let halved = Transform::affine([0.5, 0.0, 0.25, 0.0, 0.5, 0.25]);
    let cases: [(&str, Transform, &[DrizzleKernel], RangeInclusive<usize>); 2] = [
        (
            "shifted",
            shifted,
            &[DrizzleKernel::Turbo, DrizzleKernel::Square],
            2..=20,
        ),
        ("halved", halved, &DrizzleKernel::ALL, 4..=7),
    ];
    for (name, transform, kernels, interior) in cases {
        let preimage = transform.inverse();
        for &kernel in kernels {
            let product = drizzle_one(
                size,
                kernel_config(kernel, 1.0, 1.0),
                image.clone(),
                &transform,
                None,
            );
            for y in interior.clone() {
                for x in interior.clone() {
                    let source = preimage.apply(DVec2::new(x as f64, y as f64));
                    let expected = ramp(source.x, source.y);
                    let actual = f64::from(product.image.channel(0)[(x, y)]);
                    assert!(
                        (actual - expected).abs() <= bound,
                        "{name} {kernel:?} ({x}, {y}): {actual}, expected {expected}"
                    );
                }
            }
        }
    }
}
