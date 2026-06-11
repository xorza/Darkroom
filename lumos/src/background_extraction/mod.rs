//! Gradient / background extraction — model the smoothly-varying unwanted background (light
//! pollution, sky glow, moon glow, residual vignetting) on the **linear** master and remove it,
//! without eroding large-scale real signal. See `README.md` for the algorithm research, primary
//! sources, and the wider design (tiled-mesh and TPS alternatives).
//!
//! This implements the README's **safe default** (§3a): a robust tiled sky estimate fed to a
//! **low-order 2D polynomial** surface, fit by least squares with iterative outlier rejection, then
//! removed per channel. Low order is *why* it can't eat large nebulosity — a degree ≤ 4 surface
//! physically cannot represent small-scale structure, only the broad gradient.
//!
//! Distinct from background *neutralization* (`color_calibration::neutralize_background`, which only
//! equalizes per-channel offsets): this removes a **spatial surface**, per channel (light pollution
//! is coloured), and runs on the linear master *before* colour calibration and the stretch.

use common::Buffer2;
use nalgebra::{DMatrix, DVector};

use crate::io::astro_image::AstroImage;
use crate::math::statistics::sigma_clipped_median_mad;

/// How the modeled background is removed from the image.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackgroundMode {
    /// `out = in − model`. For **additive** gradients (light pollution, sky/moon glow) — the usual
    /// choice. Adds no noise (a smooth surface is noiseless) and preserves real flux differences.
    Subtract,
    /// `out = in / (model / mean(model))`, divisor floored. For **multiplicative** residuals
    /// (vignetting the master flat missed, differential absorption).
    Divide,
}

/// Parameters for [`extract_background`].
#[derive(Debug, Clone)]
pub struct BackgroundConfig {
    /// Sample-tile size in px. Each tile yields one robust sky sample. Larger → smoother, less able
    /// to absorb extended real signal; should be far larger than stars and smaller than the gradient.
    pub tile_size: usize,
    /// Polynomial degree (1–4). Capped at 4 (Siril: beyond 4 the fit is unstable). Low order is the
    /// primary guard against subtracting nebulosity.
    pub degree: usize,
    pub mode: BackgroundMode,
    /// Sample tiles whose residual from the fitted surface exceeds this many robust σ are rejected
    /// (they sit on nebulosity or unrejected stars), then the surface is refit.
    pub rejection_sigma: f32,
    /// Refit passes (each rejects residual outliers and refits). 0 = a single unrefined fit.
    pub iterations: usize,
    /// Minimum normalized divisor in [`BackgroundMode::Divide`] — caps noise amplification at
    /// `1/floor`× where the model is dark (same hazard as flat-fielding).
    pub divide_floor: f32,
}

impl Default for BackgroundConfig {
    fn default() -> Self {
        Self {
            tile_size: 128,
            degree: 2,
            mode: BackgroundMode::Subtract,
            rejection_sigma: 2.5,
            iterations: 3,
            divide_floor: 0.1,
        }
    }
}

impl BackgroundConfig {
    fn validate(&self) {
        assert!(
            (1..=4).contains(&self.degree),
            "degree must be 1..=4, got {}",
            self.degree
        );
        assert!(
            self.tile_size >= 8,
            "tile_size must be ≥ 8, got {}",
            self.tile_size
        );
        assert!(
            self.rejection_sigma > 0.0,
            "rejection_sigma must be > 0, got {}",
            self.rejection_sigma
        );
        assert!(
            self.divide_floor > 0.0 && self.divide_floor <= 1.0,
            "divide_floor must be in (0, 1], got {}",
            self.divide_floor
        );
    }
}

/// Model and remove the smooth background of `image` in place, **per channel**. Operates on linear
/// data: the output background sits at ≈0 (slightly negative on noise — kept signed, not clamped).
pub fn extract_background(image: &mut AstroImage, config: &BackgroundConfig) {
    config.validate();
    for c in 0..image.channels() {
        let model = model_channel(image.channel(c), config);
        let plane = image.channel_mut(c);
        match config.mode {
            BackgroundMode::Subtract => {
                for (p, &m) in plane.pixels_mut().iter_mut().zip(model.pixels()) {
                    *p -= m;
                }
            }
            BackgroundMode::Divide => {
                let mean = mean_of(model.pixels());
                if mean <= 0.0 {
                    continue; // degenerate model (all ≤ 0) — leave the channel untouched
                }
                let floor = config.divide_floor;
                for (p, &m) in plane.pixels_mut().iter_mut().zip(model.pixels()) {
                    *p /= (m / mean).max(floor);
                }
            }
        }
    }
}

/// The fitted background surface for a single channel, as a full-resolution plane.
pub(crate) fn model_channel(channel: &Buffer2<f32>, config: &BackgroundConfig) -> Buffer2<f32> {
    let (w, h) = (channel.width(), channel.height());
    let samples = collect_samples(channel, config.tile_size);
    let terms = poly_terms(effective_degree(samples.len(), config.degree));
    let coeffs = fit_surface(&samples, &terms, config.rejection_sigma, config.iterations);
    evaluate_plane(&coeffs, &terms, w, h)
}

/// One robust sky sample per tile, at the tile centre, with coordinates normalized to `[-1, 1]`.
#[derive(Debug, Clone, Copy)]
struct Sample {
    x: f64,
    y: f64,
    z: f64,
}

/// Robust per-tile sky estimate (sigma-clipped median, rejecting stars within the tile).
fn collect_samples(channel: &Buffer2<f32>, tile: usize) -> Vec<Sample> {
    let (w, h) = (channel.width(), channel.height());
    let px = channel.pixels();
    let mut samples = Vec::new();
    let mut scratch: Vec<f32> = Vec::with_capacity(tile * tile);
    let mut deviations: Vec<f32> = Vec::new();
    let mut ty = 0;
    while ty < h {
        let y1 = (ty + tile).min(h);
        let cy = (ty + y1 - 1) as f64 * 0.5;
        let mut tx = 0;
        while tx < w {
            let x1 = (tx + tile).min(w);
            let cx = (tx + x1 - 1) as f64 * 0.5;
            scratch.clear();
            for y in ty..y1 {
                let row = y * w;
                scratch.extend_from_slice(&px[row + tx..row + x1]);
            }
            // ±3σ clip around the median, 5 passes — the SExtractor/photutils star-rejection step.
            let stats = sigma_clipped_median_mad(&mut scratch, &mut deviations, 3.0, 5);
            samples.push(Sample {
                x: norm(cx, w),
                y: norm(cy, h),
                z: stats.median as f64,
            });
            tx += tile;
        }
        ty += tile;
    }
    samples
}

/// Largest degree whose term count `(d+1)(d+2)/2` fits within `n` samples (so the fit is determined).
fn effective_degree(n: usize, requested: usize) -> usize {
    let mut d = requested.min(4);
    while d > 0 && (d + 1) * (d + 2) / 2 > n {
        d -= 1;
    }
    d
}

/// Exponent pairs `(i, j)` for every monomial `x^i·y^j` with `i + j ≤ degree`.
fn poly_terms(degree: usize) -> Vec<(u32, u32)> {
    let mut terms = Vec::new();
    for total in 0..=degree as u32 {
        for i in 0..=total {
            terms.push((i, total - i));
        }
    }
    terms
}

/// Map a pixel/centre coordinate to `[-1, 1]` (conditioning for the least-squares fit).
fn norm(c: f64, n: usize) -> f64 {
    if n > 1 {
        2.0 * c / (n as f64 - 1.0) - 1.0
    } else {
        0.0
    }
}

/// Evaluate the polynomial at normalized `(x, y)`.
fn eval(coeffs: &DVector<f64>, terms: &[(u32, u32)], x: f64, y: f64) -> f64 {
    terms
        .iter()
        .zip(coeffs.iter())
        .map(|(&(i, j), &c)| c * x.powi(i as i32) * y.powi(j as i32))
        .sum()
}

/// Least-squares solve of the normal equations `AᵀA c = Aᵀz` for the given samples and terms.
fn solve_ls(samples: &[Sample], terms: &[(u32, u32)]) -> DVector<f64> {
    let (m, k) = (samples.len(), terms.len());
    let a = DMatrix::from_fn(m, k, |r, c| {
        let (i, j) = terms[c];
        samples[r].x.powi(i as i32) * samples[r].y.powi(j as i32)
    });
    let z = DVector::from_fn(m, |r, _| samples[r].z);
    let at = a.transpose();
    let ata = &at * &a;
    let atz = &at * z;
    ata.lu().solve(&atz).unwrap_or_else(|| DVector::zeros(k))
}

/// Fit the surface, then iteratively reject samples whose residual exceeds `kappa·σ` and refit
/// (σ = MAD-scaled residual spread). Rejects tiles sitting on nebulosity or unrejected stars.
fn fit_surface(
    samples: &[Sample],
    terms: &[(u32, u32)],
    kappa: f32,
    iterations: usize,
) -> DVector<f64> {
    let mut active: Vec<Sample> = samples.to_vec();
    let mut coeffs = solve_ls(&active, terms);
    for _ in 0..iterations {
        let residuals: Vec<f64> = active
            .iter()
            .map(|s| s.z - eval(&coeffs, terms, s.x, s.y))
            .collect();
        let sigma = robust_sigma(&residuals);
        if sigma <= 0.0 {
            break;
        }
        let thresh = kappa as f64 * sigma;
        let kept: Vec<Sample> = active
            .iter()
            .zip(&residuals)
            .filter(|(_, r)| r.abs() <= thresh)
            .map(|(&s, _)| s)
            .collect();
        if kept.len() == active.len() || kept.len() < terms.len() {
            break; // converged, or refusing to drop below a determined fit
        }
        active = kept;
        coeffs = solve_ls(&active, terms);
    }
    coeffs
}

/// MAD-scaled robust sigma of residuals (`1.4826 · median|r − median(r)|`).
fn robust_sigma(residuals: &[f64]) -> f64 {
    if residuals.is_empty() {
        return 0.0;
    }
    let median = median_f64(&mut residuals.to_vec());
    let mut dev: Vec<f64> = residuals.iter().map(|&r| (r - median).abs()).collect();
    1.482_602_2 * median_f64(&mut dev)
}

fn median_f64(v: &mut [f64]) -> f64 {
    v.sort_by(|a, b| a.total_cmp(b));
    let n = v.len();
    if n == 0 {
        0.0
    } else if n % 2 == 1 {
        v[n / 2]
    } else {
        0.5 * (v[n / 2 - 1] + v[n / 2])
    }
}

/// Render the fitted polynomial to a full-resolution plane.
fn evaluate_plane(coeffs: &DVector<f64>, terms: &[(u32, u32)], w: usize, h: usize) -> Buffer2<f32> {
    let mut px = vec![0.0f32; w * h];
    for y in 0..h {
        let ny = norm(y as f64, h);
        let row = y * w;
        for x in 0..w {
            let nx = norm(x as f64, w);
            px[row + x] = eval(coeffs, terms, nx, ny) as f32;
        }
    }
    Buffer2::new(w, h, px)
}

fn mean_of(px: &[f32]) -> f32 {
    if px.is_empty() {
        0.0
    } else {
        px.iter().sum::<f32>() / px.len() as f32
    }
}

#[cfg(test)]
mod tests;
