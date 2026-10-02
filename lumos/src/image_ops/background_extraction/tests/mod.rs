#[cfg(feature = "real-data")]
mod real_data;

use crate::image_ops::background_extraction::*;
use crate::image_ops::error::OpError;
use crate::image_ops::internals::channel_plane as channel;
use crate::image_ops::stretching::Stretch;
use crate::math::statistics::median_mut;
use crate::testing::TestRng;
use crate::testing::images::{gray_image, rgb_image as rgb};
use crate::testing::prelude::*;

fn fill(size: Size2us, f: impl Fn(usize, usize) -> f32) -> Vec<f32> {
    let mut v = vec![0.0f32; size.pixel_count()];
    for y in 0..size.height {
        for x in 0..size.width {
            v[size.index_of(Vec2us::new(x, y))] = f(x, y);
        }
    }
    v
}

fn gray(size: Size2us, f: impl Fn(usize, usize) -> f32) -> LinearImage {
    gray_image(size, fill(size, f))
}

fn mean(p: &[f32]) -> f64 {
    p.iter().map(|&v| f64::from(v)).sum::<f64>() / p.len() as f64
}

/// Largest distance from the plane's own mean: what is left of a gradient once the surface is
/// removed and the sky level kept.
fn max_dev(p: &[f32]) -> f64 {
    let m = mean(p);
    p.iter()
        .fold(0.0f64, |acc, &v| acc.max((f64::from(v) - m).abs()))
}

fn min_max(p: &[f32]) -> (f32, f32) {
    p.iter()
        .fold((f32::INFINITY, f32::NEG_INFINITY), |(lo, hi), &v| {
            (lo.min(v), hi.max(v))
        })
}

/// Residual energy about the plane's mean — the kept sky level is not residual.
fn energy(p: &[f32]) -> f64 {
    let m = mean(p);
    p.iter().map(|&v| (f64::from(v) - m).powi(2)).sum()
}

#[test]
fn poly_terms_are_correct() {
    assert_eq!(poly_terms(0), vec![(0, 0)]);
    assert_eq!(poly_terms(1), vec![(0, 0), (0, 1), (1, 0)]);
    // total 2 adds (0,2),(1,1),(2,0) → 6 terms = (2+1)(2+2)/2.
    assert_eq!(poly_terms(2).len(), 6);
    assert_eq!(poly_terms(4).len(), 15);
}

#[test]
fn effective_degree_fits_within_samples() {
    // term counts: d1=3, d2=6, d3=10. Need terms ≤ n.
    assert_eq!(effective_degree(10, 3), 3);
    assert_eq!(effective_degree(6, 3), 2); // 10 > 6, 6 ≤ 6
    assert_eq!(effective_degree(3, 3), 1); // 10,6 > 3, 3 ≤ 3
    assert_eq!(effective_degree(2, 3), 0); // even 3 > 2
    assert_eq!(effective_degree(100, 4), 4); // capped at 4
}

#[test]
fn subtract_removes_linear_gradient() {
    let size = Size2us::new(200, 160);
    // a + b·x + c·y over [0.5, 0.5+0.16+0.096] — a pure additive plane, no signal.
    let plane = |x: usize, y: usize| 0.5 + 0.0008 * x as f32 + 0.0006 * y as f32;
    let mut img = gray(size, plane);
    ExtractBackground {
        degree: 1,
        tile_size: 40,
        ..Default::default()
    }
    .apply(&mut img)
    .unwrap();
    // A degree-1 surface represents the plane exactly, so what is left is flat at the plane's
    // mean: 0.5 + 0.0008·99.5 + 0.0006·79.5 = 0.6273. The fit runs in f64 and the output is
    // stored in f32, whose spacing near 0.6 is 6e-8; 1e-6 allows for that rounding and the
    // surface's own.
    let out = channel(&img, 0);
    let px = out.pixels();
    assert!(
        max_dev(px) < 1e-6,
        "gradient removed, max dev {}",
        max_dev(px)
    );
    assert!(
        (mean(px) - 0.6273).abs() < 1e-6,
        "the sky keeps its mean level, got {}",
        mean(px)
    );
}

/// A flat sky has no gradient to remove: `Subtract` leaves it exactly as it was.
#[test]
fn subtract_leaves_a_flat_sky_unchanged() {
    let size = Size2us::new(128, 128);
    let mut img = gray(size, |_, _| 0.3125);
    ExtractBackground {
        degree: 2,
        tile_size: 32,
        ..Default::default()
    }
    .apply(&mut img)
    .unwrap();
    assert!(channel(&img, 0).pixels().iter().all(|&v| v == 0.3125));
}

#[test]
fn subtract_removes_pedestal_keeps_stars() {
    let size = Size2us::new(128, 128);
    let stars = [(10, 10), (50, 80), (100, 30), (70, 70), (20, 110)];
    let mut img = gray(
        size,
        |x, y| {
            if stars.contains(&(x, y)) { 0.95 } else { 0.3 }
        },
    );
    ExtractBackground {
        degree: 2,
        tile_size: 32,
        ..Default::default()
    }
    .apply(&mut img)
    .unwrap();
    let out = channel(&img, 0);
    // Per-tile sigma-clip rejects the lone star pixel, so the modeled sky is the flat 0.3
    // pedestal: a model with no variation removes nothing, and the star keeps its 0.95.
    assert!(
        (out[size.index_of(Vec2us::new(10, 60))] - 0.3).abs() < 0.02,
        "background stays at the sky level, got {}",
        out[size.index_of(Vec2us::new(10, 60))]
    );
    let star = out[size.index_of(Vec2us::new(10, 10))];
    assert!(
        star > 0.9,
        "star signal survives the subtraction, got {star}"
    );
}

#[test]
fn divide_corrects_quadratic_vignette() {
    let size = Size2us::new(160, 160);
    let (cx, cy) = (79.5f32, 79.5f32);
    // 1 − 0.3·(r²) ∈ [0.7, 1.0] — a smooth multiplicative falloff the master flat missed.
    let vignette = |x: usize, y: usize| {
        let (dx, dy) = (x as f32 - cx, y as f32 - cy);
        let r2 = (dx * dx + dy * dy) / (cx * cx + cy * cy);
        1.0 - 0.3 * r2
    };
    let signal = 0.5f32;
    let mut img = gray(size, |x, y| signal * vignette(x, y));
    ExtractBackground {
        degree: 2,
        tile_size: 20,
        mode: BackgroundMode::Divide,
        ..Default::default()
    }
    .apply(&mut img)
    .unwrap();
    // The quadratic vignette is exactly degree-2; dividing by the normalized model flattens it to a
    // constant (= signal·mean(vignette)).
    let (lo, hi) = min_max(channel(&img, 0).pixels());
    assert!(
        hi - lo < 0.01,
        "divide flattens the vignette: residual range {} (lo {lo} hi {hi})",
        hi - lo
    );
}

#[test]
fn higher_degree_fits_cubic_better() {
    let size = Size2us::new(180, 180);
    let cubic = |x: usize, y: usize| {
        let (nx, ny) = (x as f32 / size.width as f32, y as f32 / size.height as f32);
        0.4 + 0.2 * nx - 0.3 * nx * nx + 0.25 * nx * nx * nx + 0.15 * ny * ny * ny
    };
    let resid_energy = |degree| {
        let mut img = gray(size, cubic);
        ExtractBackground {
            degree,
            tile_size: 20,
            ..Default::default()
        }
        .apply(&mut img)
        .unwrap();
        energy(channel(&img, 0).pixels())
    };
    let e1 = resid_energy(1);
    let e3 = resid_energy(3);
    // A degree-3 surface captures the cubic's curvature; degree-1 cannot represent it at all.
    assert!(
        e3 < 0.02 * e1,
        "deg-3 leaves far less residual than deg-1: e3 {e3:.3e} vs e1 {e1:.3e}"
    );
    // deg-3 removes the cubic to a tight per-pixel residual (only tile-sampling error remains —
    // ~0.2% RMS over a ~0.15-wide range).
    let rms3 = (e3 / size.pixel_count() as f64).sqrt();
    assert!(
        rms3 < 2e-3,
        "deg-3 essentially removes the cubic: residual RMS {rms3:.2e}"
    );
}

#[test]
fn removes_independent_per_channel_gradients() {
    let size = Size2us::new(120, 100);
    // A different additive gradient in each channel (coloured light pollution).
    let r = fill(size, |x, _| 0.40 + 0.0010 * x as f32);
    let g = fill(size, |_, y| 0.30 + 0.0008 * y as f32);
    let b = fill(size, |x, y| 0.50 - 0.0005 * x as f32 + 0.0006 * y as f32);
    let mut img = rgb(size, r, g, b);
    ExtractBackground {
        degree: 1,
        tile_size: 20,
        ..Default::default()
    }
    .apply(&mut img)
    .unwrap();
    // Each channel is flat at its own mean: r 0.40 + 0.0010·59.5, g 0.30 + 0.0008·49.5,
    // b 0.50 − 0.0005·59.5 + 0.0006·49.5.
    for (c, level) in [(0, 0.4595), (1, 0.3396), (2, 0.49995)] {
        let plane = channel(&img, c);
        let px = plane.pixels();
        assert!(
            max_dev(px) < 1e-6,
            "channel {c} gradient removed, max dev {}",
            max_dev(px)
        );
        assert!(
            (mean(px) - level).abs() < 1e-6,
            "channel {c} keeps its level {level}"
        );
    }
}

#[test]
fn rejects_degree_out_of_range() {
    let mut img = gray(Size2us::new(32, 32), |_, _| 0.5);
    let err = ExtractBackground {
        degree: 7,
        ..Default::default()
    }
    .apply(&mut img)
    .unwrap_err();
    assert!(
        matches!(&err, OpError::InvalidConfig(m) if m.field == "background extraction degree"),
        "expected an InvalidConfig degree error, got {err:?}"
    );
}

#[test]
fn rank_deficient_sample_grid_is_reported_without_mutating_the_image() {
    let mut img = gray(Size2us::new(8, 64), |x, y| {
        0.2 + 0.01 * x as f32 + 0.001 * y as f32
    });
    let before = channel(&img, 0).pixels().to_vec();

    let err = ExtractBackground {
        degree: 1,
        tile_size: 8,
        iterations: 0,
        ..Default::default()
    }
    .apply(&mut img)
    .unwrap_err();

    assert!(
        matches!(
            &err,
            OpError::RankDeficient {
                operation: "background surface fit",
                rank: 2,
                required_rank: 3,
            }
        ),
        "expected the one-column sample grid's rank failure, got {err:?}"
    );
    assert_eq!(
        channel(&img, 0).pixels(),
        before,
        "a failed fit must not partially rewrite its channel"
    );
}

/// The standard chain: remove the gradient, then auto-stretch. Each auto stretch must put the
/// background median on its 0.2 target. With the sky subtracted to ≈0 both used to fall onto a
/// degenerate branch and miss it.
///
/// 129×129 is an odd count, so the median is one pixel and a monotone curve maps it exactly;
/// 1e-4 is the asinh solver's own acceptance, and STF's closed form is tighter.
#[test]
fn auto_stretches_hit_their_target_after_gradient_removal() {
    let size = Size2us::new(129, 129);
    let mut rng = TestRng::new(7);
    let sky: Vec<f32> = fill(size, |x, y| {
        let star = if (x % 31, y % 29) == (15, 14) {
            0.6
        } else {
            0.0
        };
        0.08 + 0.0004 * x as f32 + 0.0003 * y as f32 + star
    })
    .into_iter()
    .map(|v| v + 0.002 * rng.next_gaussian_f32())
    .collect();
    for stretch in [Stretch::auto_asinh(), Stretch::auto_stf()] {
        let mut img = gray_image(size, sky.clone());
        ExtractBackground::default().apply(&mut img).unwrap();
        stretch.apply(&mut img).unwrap();
        let mut px = channel(&img, 0).pixels().to_vec();
        let median = median_mut(&mut px);
        assert!(
            (median - 0.2).abs() < 1e-4,
            "{stretch:?}: background median {median}, want 0.2"
        );
    }
}
