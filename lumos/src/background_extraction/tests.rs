use super::*;
use crate::io::astro_image::{AstroImage, ImageDimensions};
use common::Vec2us;

fn fill(w: usize, h: usize, f: impl Fn(usize, usize) -> f32) -> Vec<f32> {
    let mut v = vec![0.0f32; w * h];
    for y in 0..h {
        for x in 0..w {
            v[y * w + x] = f(x, y);
        }
    }
    v
}

fn gray(w: usize, h: usize, f: impl Fn(usize, usize) -> f32) -> AstroImage {
    AstroImage::from_planar_channels(ImageDimensions::new(Vec2us::new(w, h), 1), [fill(w, h, f)])
}

fn max_abs(p: &[f32]) -> f32 {
    p.iter().fold(0.0f32, |m, &v| m.max(v.abs()))
}

fn min_max(p: &[f32]) -> (f32, f32) {
    p.iter()
        .fold((f32::INFINITY, f32::NEG_INFINITY), |(lo, hi), &v| {
            (lo.min(v), hi.max(v))
        })
}

fn energy(p: &[f32]) -> f64 {
    p.iter().map(|&v| (v as f64) * (v as f64)).sum()
}

// --- basis / determinacy helpers (pure) ---

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

// --- the operation ---

#[test]
fn subtract_removes_linear_gradient() {
    let (w, h) = (200, 160);
    // a + b·x + c·y over [0.5, 0.5+0.16+0.096] — a pure additive plane, no signal.
    let plane = |x: usize, y: usize| 0.5 + 0.0008 * x as f32 + 0.0006 * y as f32;
    let mut img = gray(w, h, plane);
    extract_background_planar(
        &mut img,
        &BackgroundConfig {
            degree: 1,
            tile_size: 40,
            ..Default::default()
        },
    );
    // A degree-1 surface represents the plane exactly; subtract leaves ≈ 0 everywhere.
    let m = max_abs(img.channel(0).pixels());
    assert!(m < 1e-3, "linear gradient removed to ~0, got max abs {m}");
}

#[test]
fn subtract_removes_pedestal_keeps_stars() {
    let (w, h) = (128, 128);
    let mut img = gray(w, h, |_, _| 0.3);
    let stars = [(10, 10), (50, 80), (100, 30), (70, 70), (20, 110)];
    {
        let px = img.channel_mut(0).pixels_mut();
        for &(x, y) in &stars {
            px[y * w + x] = 0.95;
        }
    }
    extract_background_planar(
        &mut img,
        &BackgroundConfig {
            degree: 2,
            tile_size: 32,
            ..Default::default()
        },
    );
    let out = img.channel(0).pixels();
    // Per-tile sigma-clip rejects the lone star pixel, so the modeled sky is the 0.3 pedestal:
    // background → ~0, the star (0.95 − 0.3 = 0.65) survives.
    assert!(
        out[60 * w + 10].abs() < 0.02,
        "background → ~0, got {}",
        out[60 * w + 10]
    );
    let star = out[10 * w + 10];
    assert!(
        star > 0.55,
        "star signal survives the subtraction, got {star}"
    );
}

#[test]
fn divide_corrects_quadratic_vignette() {
    let (w, h) = (160, 160);
    let (cx, cy) = (79.5f32, 79.5f32);
    // 1 − 0.3·(r²) ∈ [0.7, 1.0] — a smooth multiplicative falloff the master flat missed.
    let vignette = |x: usize, y: usize| {
        let (dx, dy) = (x as f32 - cx, y as f32 - cy);
        let r2 = (dx * dx + dy * dy) / (cx * cx + cy * cy);
        1.0 - 0.3 * r2
    };
    let signal = 0.5f32;
    let mut img = gray(w, h, |x, y| signal * vignette(x, y));
    extract_background_planar(
        &mut img,
        &BackgroundConfig {
            degree: 2,
            tile_size: 20,
            mode: BackgroundMode::Divide,
            ..Default::default()
        },
    );
    // The quadratic vignette is exactly degree-2; dividing by the normalized model flattens it to a
    // constant (= signal·mean(vignette)).
    let (lo, hi) = min_max(img.channel(0).pixels());
    assert!(
        hi - lo < 0.01,
        "divide flattens the vignette: residual range {} (lo {lo} hi {hi})",
        hi - lo
    );
}

#[test]
fn higher_degree_fits_cubic_better() {
    let (w, h) = (180, 180);
    let cubic = |x: usize, y: usize| {
        let (nx, ny) = (x as f32 / w as f32, y as f32 / h as f32);
        0.4 + 0.2 * nx - 0.3 * nx * nx + 0.25 * nx * nx * nx + 0.15 * ny * ny * ny
    };
    let resid_energy = |degree| {
        let mut img = gray(w, h, cubic);
        extract_background_planar(
            &mut img,
            &BackgroundConfig {
                degree,
                tile_size: 20,
                ..Default::default()
            },
        );
        energy(img.channel(0).pixels())
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
    let rms3 = (e3 / (w * h) as f64).sqrt();
    assert!(
        rms3 < 2e-3,
        "deg-3 essentially removes the cubic: residual RMS {rms3:.2e}"
    );
}

#[test]
fn removes_independent_per_channel_gradients() {
    let (w, h) = (120, 100);
    // A different additive gradient in each channel (coloured light pollution).
    let r = fill(w, h, |x, _| 0.40 + 0.0010 * x as f32);
    let g = fill(w, h, |_, y| 0.30 + 0.0008 * y as f32);
    let b = fill(w, h, |x, y| 0.50 - 0.0005 * x as f32 + 0.0006 * y as f32);
    let mut img =
        AstroImage::from_planar_channels(ImageDimensions::new(Vec2us::new(w, h), 3), [r, g, b]);
    extract_background_planar(
        &mut img,
        &BackgroundConfig {
            degree: 1,
            tile_size: 20,
            ..Default::default()
        },
    );
    for c in 0..3 {
        let m = max_abs(img.channel(c).pixels());
        assert!(
            m < 1e-3,
            "channel {c} gradient removed independently, max abs {m}"
        );
    }
}
