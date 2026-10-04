#![expect(
    clippy::cast_sign_loss,
    reason = "test fixtures are small images, with non-negative coordinates and offsets of a few dozen pixels"
)]

use crate::internals::prelude::*;
use crate::registration::config::{self, InterpolationMethod, WarpParams};
use crate::registration::resample;
use crate::registration::resample::internals::warp_plane;
use crate::registration::resample::kernel::LANCZOS_LUT_RESOLUTION;
use crate::registration::resample::kernel::warp_kernel::Filter;
use crate::registration::transform::{Transform, WarpTransform};

/// An integer shift samples every output pixel at a pixel centre, so it copies the source pixel
/// there, and outside the source it is the border.
///
/// Nearest, bilinear and bicubic weigh the centre tap 1 and the rest exactly 0, so they copy it
/// exactly, clamp or none: the clamp sees no negative lobe, and splits the value into its parts
/// above and below zero, one of which is zero. A Lanczos table entry at a nonzero integer is the
/// f32 kernel's rounding residue, not 0: its window lets through `leak = Σ|L(k)|, k ≠ 0` per axis
/// of every other tap's difference from the centre, `((1 + leak)² − 1)·range` in all, on top of
/// the `SIZE² + 3` roundings of each sum against the window's absolute sum. At the edges the
/// window keeps its in-bounds taps, the centre among them.
#[test]
fn integer_shifts_copy_the_source() {
    let size = Size2us::new(24, 20);
    let input = Buffer2::new(
        size.width,
        size.height,
        (0..size.pixel_count())
            .map(|i| ((i * 13 + i / size.width * 7) % 31) as f32 / 9.0 - 1.7)
            .collect(),
    );
    let range = 30.0 / 9.0;
    let largest = 1.7f32;
    for shift in [(0, 0), (2, -1), (-3, 4), (7, 0)] {
        let transform = WarpTransform::new(Transform::translation(DVec2::new(
            f64::from(shift.0),
            f64::from(shift.1),
        )));
        for method in InterpolationMethod::ALL {
            let params = config::internals::warp_params(method);
            let tolerance = match Filter::of(method) {
                Some(Filter::Lanczos(order)) => {
                    let lut = order.lut();
                    let leak: f32 = (1..=order.a())
                        .map(|k| 2.0 * lut.at((k * LANCZOS_LUT_RESOLUTION) as f32).abs())
                        .sum();
                    let taps = (4 * order.a() * order.a()) as f32;
                    let spread = (1.0 + leak) * (1.0 + leak);
                    (spread - 1.0) * range + (taps + 3.0) * f32::EPSILON * largest * spread
                }
                _ => 0.0,
            };
            let mut output = Buffer2::new_filled(size.width, size.height, f32::NAN);
            warp_plane(&input, &mut output, &transform, params);
            for y in 0..size.height {
                for x in 0..size.width {
                    let source_x = x as i32 + shift.0;
                    let source_y = y as i32 + shift.1;
                    let inside = (0..size.width as i32).contains(&source_x)
                        && (0..size.height as i32).contains(&source_y);
                    let expected = if inside {
                        input[(source_x as usize, source_y as usize)]
                    } else {
                        params.border_value
                    };
                    let actual = output[(x, y)];
                    assert!(
                        (actual - expected).abs() <= tolerance,
                        "{method:?} shift {shift:?} ({x}, {y}): {actual}, expected {expected}"
                    );
                }
            }
        }
    }
}

/// A homography's horizon maps to infinity: the column on it takes the border and has no coverage,
/// and nothing anywhere turns non-finite — at the horizon itself, and a hair short of it, where the
/// position is finite but past `i32::MAX`.
#[test]
fn a_homography_horizon_takes_the_border_and_no_coverage() {
    const WIDTH: usize = 16;
    const HEIGHT: usize = 8;
    const HORIZON_X: usize = 8;
    const BORDER: f32 = -0.25;

    let input = LinearImage::from_pixels(
        ImageDimensions::new((WIDTH, HEIGHT), 1),
        vec![0.75; WIDTH * HEIGHT],
    );
    for horizon_scale in [1.0, 1.0 - 1e-12] {
        let transform = Transform::homography([
            1.0,
            0.0,
            0.0,
            0.0,
            1.0,
            0.0,
            -horizon_scale / HORIZON_X as f64,
            0.0,
        ]);
        let wt = WarpTransform::new(transform);
        let horizon = wt.apply(DVec2::new(HORIZON_X as f64, 0.0));
        if horizon_scale == 1.0 {
            assert!(!horizon.is_finite());
        } else {
            assert!(horizon.is_finite());
            assert!(horizon.x > f64::from(i32::MAX));
        }

        for method in InterpolationMethod::ALL {
            let params = WarpParams {
                method,
                border_value: BORDER,
                ..Default::default()
            };
            let warped = resample::warp(&input, &wt, params);
            let output = warped.image.channel(0);
            let coverage = &warped.coverage;

            for y in 0..HEIGHT {
                assert_eq!(
                    output[(HORIZON_X, y)],
                    BORDER,
                    "{method:?} horizon value at y={y}"
                );
                assert_eq!(
                    coverage[(HORIZON_X, y)],
                    0.0,
                    "{method:?} horizon coverage at y={y}"
                );
            }
            assert!(
                output.pixels().iter().all(|value| value.is_finite()),
                "{method:?} produced a non-finite value"
            );
            assert!(
                coverage.pixels().iter().all(|value| value.is_finite()),
                "{method:?} produced non-finite coverage"
            );
        }
    }
}

/// An image narrower than any Lanczos window reads a constant back wherever it is sampled: every
/// window is clipped, and the in-bounds taps, normalized, or the Bilinear fallback over them, give
/// the constant to the rounding of their sums. Each sum of up to 64 terms rounds by `64·ε` of its
/// absolute sum, and a window the test admits has `Σ|L| ≤ √n·√(Σ L²) ≤ 8·Σ L`, so the ratio of
/// two of them is off by at most `2·64·ε·8` of the constant.
#[test]
fn an_image_smaller_than_the_kernel_reads_a_constant_back() {
    let size = Size2us::new(3, 3);
    let input = Buffer2::new_filled(size.width, size.height, 0.5f32);
    let wt = WarpTransform::new(Transform::translation(DVec2::new(0.3, -0.2)));
    let tolerance = 2.0 * 64.0 * f32::EPSILON * 8.0 * 0.5;
    for method in [
        InterpolationMethod::Lanczos2,
        InterpolationMethod::Lanczos3,
        InterpolationMethod::Lanczos4,
    ] {
        let mut output = Buffer2::new_default(size.width, size.height);
        warp_plane(
            &input,
            &mut output,
            &wt,
            config::internals::warp_params(method),
        );
        assert!(
            output
                .pixels()
                .iter()
                .all(|&value| (value - 0.5).abs() <= tolerance),
            "{method:?}: {:?}",
            output.pixels()
        );
    }
}
