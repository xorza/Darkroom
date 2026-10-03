use crate::stacking::registration::config::{self, InterpolationMethod, WarpParams};
use crate::stacking::registration::resample::internals::warp_plane;
use crate::stacking::registration::resample::kernel::LanczosOrder;
use crate::stacking::registration::resample::quality;
use crate::stacking::registration::transform::{Transform, WarpTransform};
use crate::testing::prelude::*;

/// An integer shift samples every output pixel at a pixel centre, so it copies the source pixel
/// there, and outside the source it is the border.
///
/// Nearest, bilinear and bicubic weigh the centre tap 1 and the rest exactly 0, so they copy it
/// exactly. A Lanczos table entry at a nonzero integer is the f32 kernel's rounding residue, not 0:
/// its window lets through `leak = Σ|L(k)|, k ≠ 0` per axis of every other tap's difference from
/// the centre, `((1 + leak)² − 1)·range` in all, on top of the row's `SIZE² + 3` roundings against
/// the window's absolute sum (see `row`'s oracle test). At the edges Lanczos falls back to
/// bilinear, which is exact.
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
            let tolerance = LanczosOrder::of(method).map_or(0.0, |order| {
                let lut = order.lut();
                let leak: f32 = (1..=order.a())
                    .map(|k| 2.0 * lut.lookup_positive(k as f32).abs())
                    .sum();
                let taps = (4 * order.a() * order.a()) as f32;
                let spread = (1.0 + leak) * (1.0 + leak);
                (spread - 1.0) * range + (taps + 3.0) * f32::EPSILON * largest * spread
            });
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

    let input = Buffer2::new_filled(WIDTH, HEIGHT, 0.75);
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
            };
            let mut output = Buffer2::new_default(WIDTH, HEIGHT);
            warp_plane(&input, &mut output, &wt, params);
            let coverage =
                quality::internals::maps(Size2us::new(WIDTH, HEIGHT), &wt, method).coverage;

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

/// An image narrower than any Lanczos window is sampled by the bilinear fallback everywhere, which
/// returns a constant exactly.
#[test]
fn an_image_smaller_than_the_kernel_falls_back_exactly() {
    let size = Size2us::new(3, 3);
    let input = Buffer2::new_filled(size.width, size.height, 0.5f32);
    let wt = WarpTransform::new(Transform::translation(DVec2::new(0.3, -0.2)));
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
            output.pixels().iter().all(|&value| value == 0.5),
            "{method:?}: {:?}",
            output.pixels()
        );
    }
}
