use crate::image_ops::wavelet::{atrous_smooth, max_scales, reflect};
use crate::internals::prelude::*;

fn pattern(size: Size2us) -> Buffer2<f32> {
    let px = (0..size.pixel_count())
        .map(|i| {
            let p = size.point_of(i);
            let (x, y) = (p.x as f32, p.y as f32);
            0.5 + 0.3 * (x * 0.3).sin() * (y * 0.2).cos()
        })
        .collect();
    Buffer2::new(size.width, size.height, px)
}

#[test]
fn reflect_mirrors_indices() {
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
    let src = Buffer2::new_filled(size.width, size.height, 0.42);
    let mut dst = Buffer2::new_default(size.width, size.height);
    let mut tmp = Buffer2::new_default(size.width, size.height);
    for step in [1usize, 2, 4] {
        atrous_smooth(&src, &mut dst, &mut tmp, step);
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
/// passes, a few ε of the largest value (0.8), held to 8ε of it.
#[test]
fn atrous_smooth_is_the_b3_convolution() {
    let size = Size2us::new(17, 13);
    let src = pattern(size);
    let weights = [1.0f64, 4.0, 6.0, 4.0, 1.0].map(|w| w / 16.0);
    let mut dst = Buffer2::new_default(size.width, size.height);
    let mut tmp = Buffer2::new_default(size.width, size.height);
    let bound = 8.0 * f64::from(f32::EPSILON) * 0.8;
    for step in [1usize, 2, 4, 8] {
        atrous_smooth(&src, &mut dst, &mut tmp, step);
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
