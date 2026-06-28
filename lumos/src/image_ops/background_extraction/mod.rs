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

use imaginarium::Buffer2;
use nalgebra::{DMatrix, DVector};
use rayon::prelude::*;

use crate::background_mesh::TileGrid;
use crate::image_ops::op::{OpError, ensure, require_f32_master};
use crate::image_ops::process_planes;
use crate::math::statistics::MAD_TO_SIGMA;
use imaginarium::Image;

/// Sigma-clip passes for the per-tile sky estimate (matches the detector's tiled-background default).
const SKY_CLIP_ITERATIONS: usize = 3;

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

/// Model and remove the smooth background of an image in place, **per channel**. Operates on linear
/// data: the output background sits at ≈0 (slightly negative on noise — kept signed, not clamped).
#[derive(Debug, Clone)]
pub struct ExtractBackground {
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

impl Default for ExtractBackground {
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

impl ExtractBackground {
    /// Set the sample-tile size in px.
    pub fn tile_size(mut self, tile_size: usize) -> Self {
        self.tile_size = tile_size;
        self
    }

    /// Set the polynomial degree (1–4).
    pub fn degree(mut self, degree: usize) -> Self {
        self.degree = degree;
        self
    }

    /// Set subtract-vs-divide removal.
    pub fn mode(mut self, mode: BackgroundMode) -> Self {
        self.mode = mode;
        self
    }

    /// Set the tile-rejection sigma.
    pub fn rejection_sigma(mut self, rejection_sigma: f32) -> Self {
        self.rejection_sigma = rejection_sigma;
        self
    }

    /// Set the refit-pass count.
    pub fn iterations(mut self, iterations: usize) -> Self {
        self.iterations = iterations;
        self
    }

    /// Set the minimum normalized divisor for [`BackgroundMode::Divide`].
    pub fn divide_floor(mut self, divide_floor: f32) -> Self {
        self.divide_floor = divide_floor;
        self
    }

    /// Model and remove the smooth background of `image` in place, per channel.
    ///
    /// # Errors
    /// [`OpError::UnsupportedFormat`] unless `image` is `L_F32`/`RGB_F32`; [`OpError::InvalidConfig`]
    /// on out-of-range parameters.
    pub fn apply(&self, image: &mut Image) -> Result<(), OpError> {
        self.validate()?;
        require_f32_master(image)?;
        process_planes(image, |planes| extract_background_core(planes, self));
        Ok(())
    }

    fn validate(&self) -> Result<(), OpError> {
        ensure((1..=4).contains(&self.degree), || {
            format!("degree must be 1..=4, got {}", self.degree)
        })?;
        ensure(self.tile_size >= 8, || {
            format!("tile_size must be ≥ 8, got {}", self.tile_size)
        })?;
        ensure(self.rejection_sigma > 0.0, || {
            format!("rejection_sigma must be > 0, got {}", self.rejection_sigma)
        })?;
        ensure(self.divide_floor > 0.0 && self.divide_floor <= 1.0, || {
            format!("divide_floor must be in (0, 1], got {}", self.divide_floor)
        })
    }
}

/// The per-channel background fit + removal, over channel planes (1 for L, 3 for RGB).
fn extract_background_core(planes: &mut [Buffer2<f32>], config: &ExtractBackground) {
    for plane in planes.iter_mut() {
        let model = model_channel(plane, config);
        match config.mode {
            BackgroundMode::Subtract => {
                plane
                    .pixels_mut()
                    .par_iter_mut()
                    .zip(model.pixels().par_iter())
                    .for_each(|(p, &m)| *p -= m);
            }
            BackgroundMode::Divide => {
                let mean = mean_of(model.pixels());
                if mean <= 0.0 {
                    continue; // degenerate model (all ≤ 0) — leave the channel untouched
                }
                let floor = config.divide_floor;
                plane
                    .pixels_mut()
                    .par_iter_mut()
                    .zip(model.pixels().par_iter())
                    .for_each(|(p, &m)| *p /= (m / mean).max(floor));
            }
        }
    }
}

/// The fitted background surface for a single channel, as a full-resolution plane.
fn model_channel(channel: &Buffer2<f32>, config: &ExtractBackground) -> Buffer2<f32> {
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

/// One robust sky sample per tile — the tile centre with coordinates normalized to `[-1, 1]` and the
/// shared [`TileGrid`] SExtractor-style sky estimate (per-tile ±σ-clip → Pearson mode). Reuses the
/// exact estimator star detection uses, so the gradient fit and the detector see the same sky. The
/// grid 3×3 median filter is **off** (it would bias a real gradient's boundary tiles; outlier tiles
/// are instead rejected by the surface fit's residual clip). A `None` object mask for now — passing a
/// star/bright-signal mask here is the §6.2 refinement.
fn collect_samples(channel: &Buffer2<f32>, tile: usize) -> Vec<Sample> {
    let (w, h) = (channel.width(), channel.height());
    let tile = tile.min(w).min(h).max(1);
    let mut grid = TileGrid::new_uninit(w, h, tile);
    grid.compute(channel, None, SKY_CLIP_ITERATIONS, false);

    let mut samples = Vec::with_capacity(grid.tiles_x() * grid.tiles_y());
    for ty in 0..grid.tiles_y() {
        let y = norm(grid.center_y(ty) as f64, h);
        for (tx, &cx) in grid.centers_x.iter().enumerate() {
            samples.push(Sample {
                x: norm(cx as f64, w),
                y,
                z: grid.get(tx, ty).sky as f64,
            });
        }
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
    f64::from(MAD_TO_SIGMA) * median_f64(&mut dev)
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

/// Render the fitted polynomial to a full-resolution plane, parallel over rows.
///
/// Rather than re-evaluate the bivariate polynomial per pixel (a `powi` per term), the coefficients
/// are packed into a `(degree+1)²` matrix `C[i][j]`. For each row `y` the powers `y^j` collapse `C`
/// into a 1-D polynomial in `x` (`b[i] = Σ_j C[i][j]·y^j`), which every pixel in the row evaluates by
/// Horner — `degree` fused multiply-adds, no `powi`.
fn evaluate_plane(coeffs: &DVector<f64>, terms: &[(u32, u32)], w: usize, h: usize) -> Buffer2<f32> {
    let degree = terms
        .iter()
        .map(|&(i, j)| (i + j) as usize)
        .max()
        .unwrap_or(0);
    // `effective_degree` caps the surface at 4, so `degree + 1 ≤ 5` rows/cols fit fixed buffers.
    assert!(degree <= 4, "background surface degree {degree} exceeds 4");
    let d1 = degree + 1;
    let mut c_mat = [0.0f64; 25]; // (degree+1)² ≤ 25, row-major `[i*d1 + j]`
    for (&(i, j), &c) in terms.iter().zip(coeffs.iter()) {
        c_mat[i as usize * d1 + j as usize] = c;
    }

    let mut px = vec![0.0f32; w * h];
    px.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
        let ny = norm(y as f64, h);
        let mut yp = [0.0f64; 5];
        yp[0] = 1.0;
        for j in 1..d1 {
            yp[j] = yp[j - 1] * ny;
        }
        // Collapse the y dimension: b[i] = Σ_j C[i][j]·y^j.
        let mut b = [0.0f64; 5];
        for i in 0..d1 {
            b[i] = (0..d1).map(|j| c_mat[i * d1 + j] * yp[j]).sum();
        }
        for (x, p) in row.iter_mut().enumerate() {
            let nx = norm(x as f64, w);
            let mut acc = b[degree];
            for i in (0..degree).rev() {
                acc = acc * nx + b[i];
            }
            *p = acc as f32;
        }
    });
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
