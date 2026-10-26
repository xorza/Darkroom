use common::Buffer2;

use crate::math::wavelet::{
    B3, SmoothRow, atrous_smooth, atrous_smooth_serial, max_scales, reflect, white_noise_sigma,
};
use common::Size2us;
use simd::IsaTier;

fn pattern(size: Size2us) -> Buffer2<f32> {
    Buffer2::from_fn(size, |x, y| {
        let (x, y) = (x as f32, y as f32);
        0.5 + 0.3 * (x * 0.3).sin() * (y * 0.2).cos()
    })
}

#[test]
fn reflect_mirrors_idxs() {
    let n = 5; // period = 2*(5-1) = 8
    assert_eq!(reflect(0, n), 0);
    assert_eq!(reflect(4, n), 4);
    assert_eq!(reflect(-1, n), 1);
    assert_eq!(reflect(-2, n), 2);
    assert_eq!(reflect(-3, n), 3);
    assert_eq!(reflect(5, n), 3);
    assert_eq!(reflect(6, n), 2);
    assert_eq!(reflect(7, n), 1);
    assert_eq!(reflect(8, n), 0);
    assert_eq!(reflect(10, n), 2);
    assert_eq!(reflect(-10, n), 2);
    assert_eq!(reflect(3, 1), 0);
    assert_eq!(reflect(-3, 1), 0);
}

#[test]
fn max_scales_bounds_by_dimension() {
    assert_eq!(max_scales(Size2us::new(1, 1)), 1);
    assert_eq!(max_scales(Size2us::new(2, 2)), 1);
    assert_eq!(max_scales(Size2us::new(5, 5)), 2);
    assert_eq!(max_scales(Size2us::new(8, 8)), 3);
    // Bounded by the smaller axis, whichever it is.
    assert_eq!(max_scales(Size2us::new(1000, 8)), 3);
    assert_eq!(max_scales(Size2us::new(8, 1000)), 3);
}

#[test]
fn atrous_smooth_preserves_constant() {
    // The B3 kernel sums to 1, so a flat field is reproduced exactly at every hole spacing.
    let size = Size2us::new(8, 6);
    let src = Buffer2::new_filled(size, 0.42);
    let mut dst = Buffer2::new_default(size);
    for step in [1usize, 2, 4] {
        atrous_smooth(&src, &mut dst, step);
        // The weights are dyadic and sum to 1; the two passes round 0.42 by a few ulps at most.
        for &v in dst.pixels() {
            assert!(
                (v - 0.42).abs() <= 4.0 * f32::EPSILON * 0.42,
                "constant preserved at step {step}: {v}"
            );
        }
    }
}

/// One smoothing step against the B3 à trous convolution written out in f64: the kernel `(1, 4, 6,
/// 4, 1)/16` on both axes, taps `step` apart, every out-of-frame tap mirrored without repeating the
/// edge — at every pixel, for hole spacings that stay inside the frame and ones that reach past it
/// on both sides (17 × 13 against a reach of 2·8). The f32 path rounds each of its two five-tap
/// passes, a few ε of the largest value (0.8), held to 8ε of it. The serial form gives the
/// parallel form's bits, through a row of scratch that starts the wrong length and dirty.
#[test]
fn atrous_smooth_is_the_b3_convolution() {
    let size = Size2us::new(17, 13);
    let src = pattern(size);
    let weights = [1.0f64, 4.0, 6.0, 4.0, 1.0].map(|w| w / 16.0);
    let mut dst = Buffer2::new_default(size);
    let mut serial = Buffer2::new_default(size);
    let mut col = vec![9.0; 3];
    let bound = 8.0 * f64::from(f32::EPSILON) * 0.8;
    for step in [1usize, 2, 4, 8] {
        atrous_smooth(&src, &mut dst, step);
        atrous_smooth_serial(&src, &mut serial, step, &mut col);
        assert!(
            serial
                .pixels()
                .iter()
                .zip(dst.pixels())
                .all(|(a, b)| a.to_bits() == b.to_bits()),
            "step {step}: the serial form differs"
        );
        for y in 0..size.height {
            for x in 0..size.width {
                let mut expected = 0.0f64;
                for (j, wy) in weights.iter().enumerate() {
                    let sy = reflect(
                        y as isize + (j as isize - 2) * step as isize,
                        size.height as isize,
                    );
                    for (i, wx) in weights.iter().enumerate() {
                        let sx = reflect(
                            x as isize + (i as isize - 2) * step as isize,
                            size.width as isize,
                        );
                        expected += wx * wy * f64::from(src[(sx, sy)]);
                    }
                }
                let got = f64::from(dst[(x, y)]);
                assert!(
                    (got - expected).abs() <= bound,
                    "step {step} at ({x}, {y}): {got} vs {expected}"
                );
            }
        }
    }
}

/// The layers' white-noise σ against the same closed form evaluated in NumPy, and so against the
/// published 0.889, 0.200, 0.086, 0.041, 0.020 to their three digits. The first by hand:
/// `Σw₀² = 1 − 2·(6/16)² + (70/256)² = 0.793517`, whose root is 0.890796. The sums are of a few
/// dozen products of dyadic fractions, so f64 holds 1e-15.
#[test]
fn layer_sigmas_have_their_closed_form() {
    let numpy = [
        0.890_796_310_278_758_4,
        0.200_663_851_024_418_97,
        0.085_507_504_753_369_93,
        0.041_217_444_374_316_2,
        0.020_424_966_592_781_434,
        0.010_189_759_249_213_292,
    ];
    for (scale, expected) in numpy.into_iter().enumerate() {
        assert!(
            (white_noise_sigma(scale) - expected).abs() < 1e-15,
            "scale {scale}: {}",
            white_noise_sigma(scale)
        );
    }
}

/// Every tier's smooth is the scalar one bit for bit: each output the five products `w·v`, each
/// rounded, added left to right. Widths from 1 to 70 cross the four-vector block, the single
/// vector and the scalar tail, and the reflected borders; steps 1, 2 and 4 move the interior.
#[test]
fn every_tier_smooths_a_row_as_the_scalar_order_does() {
    let reference = |row: &[f32], x: usize, step: usize| {
        let (xi, s, wi) = (x as isize, step as isize, row.len() as isize);
        B3[0] * row[reflect(xi - 2 * s, wi)]
            + B3[1] * row[reflect(xi - s, wi)]
            + B3[2] * row[x]
            + B3[3] * row[reflect(xi + s, wi)]
            + B3[4] * row[reflect(xi + 2 * s, wi)]
    };
    for width in 1..=70 {
        let rows: Vec<Vec<f32>> = (0..5)
            .map(|row| {
                (0..width)
                    .map(|x| 0.5 + 0.3 * ((x * 7 + row * 13) as f32 * 0.37).sin())
                    .collect()
            })
            .collect();
        let col: Vec<f32> = (0..width)
            .map(|x| {
                B3[0] * rows[0][x]
                    + B3[1] * rows[1][x]
                    + B3[2] * rows[2][x]
                    + B3[3] * rows[3][x]
                    + B3[4] * rows[4][x]
            })
            .collect();
        for step in [1, 2, 4] {
            let expected: Vec<u32> = (0..width)
                .map(|x| reference(&col, x, step).to_bits())
                .collect();
            for tier in IsaTier::supported() {
                let (mut scratch, mut out) = (vec![0.0; width], vec![0.0; width]);
                tier.run(SmoothRow {
                    rows: [&rows[0], &rows[1], &rows[2], &rows[3], &rows[4]],
                    col: &mut scratch,
                    out: &mut out,
                    step,
                });
                let got: Vec<u32> = out.iter().map(|value| value.to_bits()).collect();
                assert_eq!(got, expected, "{tier} width {width} step {step}");
            }
        }
    }
}
