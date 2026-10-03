//! Tests for background estimation.

mod synthetic_skies;

use crate::internals::prelude::*;
use crate::internals::synthetic::background_map;
use crate::math::statistics::mad_to_sigma;
use crate::star_detection::background::background_estimate::{BackgroundEstimate, Refinement};
use crate::star_detection::config::background_config::BackgroundConfig;
use crate::star_detection::resources::DetectionResources;

/// The background and noise maps of `pixels` at `tile_size`.
fn estimate(pixels: &Buffer2<f32>, tile_size: usize) -> BackgroundEstimate {
    background_map::estimate(
        pixels,
        &BackgroundConfig {
            tile_size,
            ..Default::default()
        },
    )
}

/// A constant frame maps to its own value at every pixel and to no noise, bit for bit: every tile
/// reads the value, the spline's rise between equal nodes is 0, and its curvature terms are 0 —
/// over one tile, one row or column of tiles, a tile clamped to a small frame, and many.
#[test]
fn constant_skies_come_back_exactly() {
    for (width, height, tile_size, value) in [
        (128, 128, 32, 0.5f32),
        (20, 20, 64, 0.7),
        (256, 64, 32, 0.4),
        (32, 32, 32, 0.42),
        (32, 128, 32, 0.42),
        (128, 32, 32, 0.77),
        (256, 256, 64, 0.33),
        (128, 128, 16, 0.5),
        (128, 128, 128, 0.5),
    ] {
        let background = estimate(&Buffer2::new_filled(width, height, value), tile_size);
        let case = format!("{width}×{height} at tile {tile_size}");
        assert_eq!(background.background.width(), width, "{case}");
        assert_eq!(background.background.height(), height, "{case}");
        assert!(
            background.background.pixels().iter().all(|&v| v == value),
            "{case}: background off {value}"
        );
        assert!(
            background.noise.pixels().iter().all(|&v| v == 0.0),
            "{case}: noise off 0"
        );
    }
}

/// A plane sky comes back as itself at every pixel, past the outer tile centres too and over the
/// partial tiles a frame not a multiple of the tile leaves: each tile's samples are point-symmetric
/// about its centre, so its sky is the plane there; the 3×3 median keeps a plane; and the natural
/// spline through a plane's nodes is that plane, its end intervals continuing it. What is left is
/// f32 rounding — of the samples, the spline's rise and its products, twice over (y, then x) —
/// held to 8ε of the largest value.
#[test]
fn plane_skies_come_back_exactly() {
    for (width, height, tile_size, gradient) in [
        (256, 192, 64, Vec2::new(1e-3, 0.0)),
        (256, 192, 64, Vec2::new(0.0, 1e-3)),
        (256, 192, 64, Vec2::new(1e-3, 2e-3)),
        (200, 150, 64, Vec2::new(-1e-3, 1.5e-3)),
        (160, 160, 32, Vec2::new(2e-3, -1e-3)),
    ] {
        let plane = |x: usize, y: usize| 0.5 + gradient.x * x as f32 + gradient.y * y as f32;
        let pixels = Buffer2::new(
            width,
            height,
            (0..width * height)
                .map(|i| plane(i % width, i / width))
                .collect(),
        );
        let background = estimate(&pixels, tile_size);
        let largest = pixels.pixels().iter().fold(0.0f32, |m, &v| m.max(v.abs()));
        let bound = 8.0 * f32::EPSILON * largest;
        for y in 0..height {
            for x in 0..width {
                let error = (background.background[(x, y)] - plane(x, y)).abs();
                assert!(
                    error <= bound,
                    "{width}×{height}, gradient {gradient}: ({x}, {y}) off by {error} > {bound}"
                );
            }
        }
    }
}

/// The second derivatives of the natural cubic spline through `(xs, ys)`, by the textbook
/// tridiagonal system in f64: `h₋·m₋ + 2(h₋ + h₊)·m + h₊·m₊ = 6·(slope₊ − slope₋)`, `m` 0 at both
/// ends.
fn natural_spline(xs: &[f64], ys: &[f64]) -> Vec<f64> {
    let n = xs.len();
    let mut m = vec![0.0; n];
    let (mut diagonal, mut rhs) = (vec![0.0; n], vec![0.0; n]);
    for i in 1..n - 1 {
        let (left, right) = (xs[i] - xs[i - 1], xs[i + 1] - xs[i]);
        diagonal[i] = 2.0 * (left + right);
        rhs[i] = 6.0 * ((ys[i + 1] - ys[i]) / right - (ys[i] - ys[i - 1]) / left);
        if i > 1 {
            let factor = left / diagonal[i - 1];
            diagonal[i] -= factor * left;
            rhs[i] -= factor * rhs[i - 1];
        }
    }
    for i in (1..n - 1).rev() {
        m[i] = (rhs[i] - (xs[i + 1] - xs[i]) * m[i + 1]) / diagonal[i];
    }
    m
}

/// That spline at `x`, the end intervals' cubics continued past the outer knots.
fn spline_at(xs: &[f64], ys: &[f64], m: &[f64], x: f64) -> f64 {
    let k = xs[1..xs.len() - 1]
        .iter()
        .take_while(|&&knot| knot <= x)
        .count();
    let h = xs[k + 1] - xs[k];
    let a = (xs[k + 1] - x) / h;
    let b = 1.0 - a;
    a * ys[k] + b * ys[k + 1] + ((a.powi(3) - a) * m[k] + (b.powi(3) - b) * m[k + 1]) * h * h / 6.0
}

/// Along one axis, five tiles of 32 px hold a checkerboard `vᵢ ± aᵢ`, the same across the other
/// axis: each tile's median and mean are `vᵢ`, its MAD `aᵢ`, so its sky is `vᵢ` and its σ
/// `mad_to_sigma(aᵢ)`, exactly (dyadic values, exact sums). Both sequences rise, which the 3×3
/// median keeps. The maps along that axis are then the natural cubic splines through those tile
/// values at the tile centres 15.5 + 32i — checked against an independent f64 spline at every
/// pixel, the end intervals' extrapolation included, and constant across. The noise map is that
/// spline clipped to the tile σ's range, which its extrapolation leaves at both ends. The f32
/// spline differs from the f64 one by the rounding of its solve and evaluation, a few ulps of the
/// values: ≤ 2.6e-8 measured, held to 4ε of the largest sky, 0.75.
#[test]
fn maps_follow_the_natural_spline_through_the_tile_skies() {
    const SKY: [f32; 5] = [0.25, 0.3125, 0.375, 0.5, 0.75];
    const SPREAD: [f32; 5] = [
        1.0 / 128.0,
        2.0 / 128.0,
        4.0 / 128.0,
        5.0 / 128.0,
        8.0 / 128.0,
    ];
    let centres: Vec<f64> = (0..5).map(|i| 15.5 + 32.0 * f64::from(i)).collect();
    let skies: Vec<f64> = SKY.iter().map(|&v| f64::from(v)).collect();
    let sigmas: Vec<f64> = SPREAD.iter().map(|&a| f64::from(mad_to_sigma(a))).collect();
    let (sky_m, sigma_m) = (
        natural_spline(&centres, &skies),
        natural_spline(&centres, &sigmas),
    );

    let bound = 4.0 * f64::from(f32::EPSILON) * 0.75;
    for along_x in [true, false] {
        let (width, height) = if along_x { (160, 96) } else { (96, 160) };
        let pixels = Buffer2::new(
            width,
            height,
            (0..width * height)
                .map(|i| {
                    let (x, y) = (i % width, i / width);
                    let tile = if along_x { x } else { y } / 32;
                    let sign = if (x + y) % 2 == 0 { 1.0 } else { -1.0 };
                    SKY[tile] + sign * SPREAD[tile]
                })
                .collect(),
        );
        let background = estimate(&pixels, 32);
        for y in 0..height {
            for x in 0..width {
                let at = if along_x { x } else { y } as f64;
                let sky = spline_at(&centres, &skies, &sky_m, at);
                let sigma = spline_at(&centres, &sigmas, &sigma_m, at).clamp(sigmas[0], sigmas[4]);
                let axis = if along_x { "x" } else { "y" };
                let (got_sky, got_sigma) = (
                    f64::from(background.background[(x, y)]),
                    f64::from(background.noise[(x, y)]),
                );
                assert!(
                    (got_sky - sky).abs() <= bound,
                    "along {axis}: sky at ({x}, {y}) {got_sky} vs {sky}"
                );
                assert!(
                    (got_sigma - sigma).abs() <= bound,
                    "along {axis}: σ at ({x}, {y}) {got_sigma} vs {sigma}"
                );
            }
        }
    }
}

/// Outliers in under half of a tile's pixels leave its sky exact: a tenth of the frame at 0.95 on
/// a 0.2 sky gives a MAD of 0, so the clip keeps only the 0.2s, whose median and mean are 0.2.
#[test]
fn outliers_in_under_half_a_tile_leave_the_sky_exact() {
    let mut pixels = Buffer2::new_filled(64, 64, 0.2f32);
    for i in (0..64 * 64).step_by(10) {
        pixels[i] = 0.95;
    }
    let background = estimate(&pixels, 32);
    assert!(background.background.pixels().iter().all(|&v| v == 0.2));
}

/// `refine` masks what stands above the sky, dilated, and measures the sky again from the rest.
///
/// Every 32×32 tile holds a checkerboard `v ± a` (v = 0.25, a = 1/64) with a 4×4 core 8 above it
/// and a 2-px halo 2a above it around the core. Unmasked, each tile counts 480 pixels at v − a and
/// 504 at v + a below the core and the halo's highs, so its median is v + a; its MAD is 2a, which
/// keeps the halo through the clip; and its mean sits 0.905a from the median, past the 0.3σ Pearson
/// bound, so the sky is the median: v + a in every tile, which no median filter can undo. The mask
/// stands at v + a + 4σ = v + 12.9a, which only the core clears. Undilated, the halo stays and the
/// sky with it; dilated by 2 the mask covers exactly the 8×8 halo, leaving 480 of each checker
/// value: the sky is v and σ is `mad_to_sigma(a)`, exactly, and a second pass, whose mask at v +
/// 5.9a is the same, keeps them.
#[test]
fn refine_masks_the_stars_and_their_halos_out() {
    const SKY: f32 = 0.25;
    const SPREAD: f32 = 1.0 / 64.0;
    let size = Size2us::new(128, 128);
    let pixels = Buffer2::new(
        size.width,
        size.height,
        (0..size.pixel_count())
            .map(|i| {
                let (x, y) = (i % size.width, i / size.width);
                let sign = if (x + y) % 2 == 0 { 1.0 } else { -1.0 };
                let lift = match (x % 32, y % 32) {
                    (14..18, 14..18) => 8.0,
                    (12..20, 12..20) => 2.0 * SPREAD,
                    _ => 0.0,
                };
                SKY + sign * SPREAD + lift
            })
            .collect(),
    );
    let config = BackgroundConfig {
        tile_size: 32,
        ..Default::default()
    };
    let refined = |iterations, mask_dilation| {
        let mut resources = DetectionResources::new(size);
        let mut background = BackgroundEstimate::estimate(&pixels, &config, &mut resources);
        background.refine(
            &pixels,
            &config,
            Refinement {
                iterations,
                mask_dilation,
            },
            4.0,
            &mut resources,
        );
        background
    };

    let unrefined = estimate(&pixels, 32);
    assert!(
        unrefined
            .background
            .pixels()
            .iter()
            .all(|&v| v == SKY + SPREAD)
    );
    let undilated = refined(1, 0);
    assert!(
        undilated
            .background
            .pixels()
            .iter()
            .all(|&v| v == SKY + SPREAD)
    );
    for iterations in [1, 2] {
        let clean = refined(iterations, 2);
        assert!(
            clean.background.pixels().iter().all(|&v| v == SKY),
            "{iterations} passes"
        );
        assert!(
            clean
                .noise
                .pixels()
                .iter()
                .all(|&v| v == mad_to_sigma(SPREAD)),
            "{iterations} passes"
        );
    }
}

#[test]
fn repeated_estimation_and_dimension_reset_preserve_exact_results() {
    let pixels = Buffer2::new(
        96,
        64,
        (0..64)
            .flat_map(|y| (0..96).map(move |x| (3 * x + y) as f32 / 512.0))
            .collect(),
    );
    let config = BackgroundConfig {
        tile_size: 16,
        ..Default::default()
    };
    let mut resources = DetectionResources::new(Size2us::new(96, 64));

    let first = BackgroundEstimate::estimate(&pixels, &config, &mut resources);
    let expected_background = first.background.pixels().to_vec();
    let expected_noise = first.noise.pixels().to_vec();
    first.release_to_pool(&mut resources);

    let second = BackgroundEstimate::estimate(&pixels, &config, &mut resources);
    assert_eq!(second.background.pixels(), expected_background);
    assert_eq!(second.noise.pixels(), expected_noise);
    second.release_to_pool(&mut resources);

    resources.reset(Size2us::new(48, 32));
    let resized_pixels = Buffer2::new_filled(48, 32, 0.25);
    let resized = BackgroundEstimate::estimate(&resized_pixels, &config, &mut resources);
    assert_eq!(resized.background.width(), 48);
    assert_eq!(resized.background.height(), 32);
    assert!(
        resized
            .background
            .pixels()
            .iter()
            .all(|&value| value == 0.25)
    );
    assert!(resized.noise.pixels().iter().all(|&value| value == 0.0));
}

#[test]
fn invalid_tile_sizes_return_exact_errors() {
    for value in [8, 512] {
        let config = BackgroundConfig {
            tile_size: value,
            ..Default::default()
        };
        let invalid = config.validate().unwrap_err();
        assert_eq!((invalid.field, invalid.value), ("tile_size", value as f64));
    }
}
