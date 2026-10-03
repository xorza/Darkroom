//! Gradient / background extraction — model the smoothly-varying unwanted background (light
//! pollution, sky glow, moon glow, residual vignetting) on the **linear** master and remove it,
//! without eroding large-scale real signal.
//!
//! The **safe default** over a tiled mesh or thin-plate splines: a robust tiled sky estimate fed
//! to a **low-order 2D polynomial** surface, fit by least squares with iterative outlier rejection,
//! then removed per channel. Low order is *why* it can't eat large nebulosity — a degree ≤ 4
//! surface physically cannot represent small-scale structure, only the broad gradient.
//!
//! Distinct from background *neutralization* (`color_calibration::neutralize_background`, which
//! only equalizes per-channel offsets): this removes a **spatial surface**, per channel (light
//! pollution is coloured), and runs on the linear master *before* colour calibration and the
//! stretch.

use common::{Introspect, IntrospectEnum};
use imaginarium::Buffer2;
use nalgebra::{DMatrix, DVector};
use rayon::prelude::*;

use crate::background_mesh::workspace::MeshWorkspace;
use crate::error::InvalidConfigField;
use crate::image_ops::error::OpError;
use crate::io::image::linear::LinearImage;
use crate::math::size2us::Size2us;
use crate::math::statistics::robust_sigma_f64;
use crate::stacking::star_detection::config::background_config::DEFAULT_SIGMA_CLIP_ITERATIONS;

/// How the modeled background is removed from the image.
///
/// `type_id` is this enum's identity to an introspecting consumer; that
/// consumer stores it, so it is fixed for the life of the type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, IntrospectEnum)]
#[config(type_id = "ed416b2d-378b-4eb1-9029-bc7a80a509aa")]
pub enum BackgroundMode {
    /// `out = in − (model − mean(model))`. For **additive** gradients (light pollution, sky/moon
    /// glow) — the usual choice. Removes the variation and keeps the sky level. Adds no noise (a
    /// smooth surface is noiseless) and preserves real flux differences.
    Subtract,
    /// `out = in / (model / mean(model))`, divisor floored. For **multiplicative** residuals
    /// (vignetting the master flat missed, differential absorption).
    Divide,
}

/// Model and remove the smooth background of an image in place, **per channel**. Operates on linear
/// data: the output background sits at each channel's mean sky level (`Subtract`) or keeps it
/// (`Divide`), with the gradient gone.
#[derive(Debug, Clone, Introspect)]
#[config(type_id = "47a71876-5db9-45f9-a21d-cc2ce40a80f2")]
pub struct ExtractBackground {
    /// Sample-tile size in px. Each tile yields one robust sky sample. Larger → smoother, less able
    /// to absorb extended real signal; should be far larger than stars and smaller than the
    /// gradient.
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
    /// Model and remove the smooth background of `image` in place, per channel.
    ///
    /// # Errors
    /// [`OpError::InvalidConfig`] on out-of-range parameters; [`OpError::RankDeficient`] when the
    /// sample geometry cannot determine the requested polynomial.
    pub fn apply(&self, image: &mut LinearImage) -> Result<(), OpError> {
        self.validate()?;
        let mut workspace = MeshWorkspace::default();
        for plane in image.planes_mut() {
            extract_background_plane(plane, self, &mut workspace)?;
        }
        Ok(())
    }

    fn validate(&self) -> Result<(), InvalidConfigField> {
        InvalidConfigField::check(
            (1..=4).contains(&self.degree),
            "background extraction degree",
            "between 1 and 4",
            self.degree as f64,
        )?;
        InvalidConfigField::check(
            self.tile_size >= 8,
            "background extraction tile_size",
            "at least 8",
            self.tile_size as f64,
        )?;
        InvalidConfigField::finite(
            "background extraction rejection_sigma",
            "finite and positive",
            self.rejection_sigma,
            |value| value > 0.0,
        )?;
        InvalidConfigField::finite(
            "background extraction divide_floor",
            "finite and in (0, 1]",
            self.divide_floor,
            |value| value > 0.0 && value <= 1.0,
        )
    }
}

/// Fit and remove the background surface of one channel plane, in place. The model is
/// evaluated on the fly inside the removal pass ([`Surface::remove`]) — a degree ≤ 4
/// polynomial needs no full-resolution model plane.
fn extract_background_plane(
    plane: &mut Buffer2<f32>,
    config: &ExtractBackground,
    workspace: &mut MeshWorkspace,
) -> Result<(), OpError> {
    let samples = collect_samples(plane, config.tile_size, workspace);
    let terms = poly_terms(effective_degree(samples.len(), config.degree));
    let coeffs = fit_surface(&samples, &terms, config.rejection_sigma, config.iterations)?;
    let surface = Surface::new(&coeffs, &terms, Size2us::new(plane.width(), plane.height()));
    match config.mode {
        BackgroundMode::Subtract => {
            // Only the model's variation is removed; its mean stays as the sky pedestal, as Siril
            // does. Without it the sky sits at ≈0 and the next step that measures the background
            // against zero (an auto stretch) has nothing to place. Each channel keeps its own
            // level, like `Divide` — neutralizing the sky colour is a separate op.
            let pedestal = surface.mean() as f32;
            surface.remove(plane, |p, m| p - (m - pedestal));
        }
        BackgroundMode::Divide => {
            let mean = surface.mean();
            if mean <= 0.0 {
                return Ok(()); // degenerate model (mean ≤ 0) — leave the channel untouched
            }
            let (mean, floor) = (mean as f32, config.divide_floor);
            surface.remove(plane, |p, m| p / (m / mean).max(floor));
        }
    }
    Ok(())
}

/// One robust sky sample per tile, at the tile centre, with coordinates normalized to `[-1, 1]`.
#[derive(Debug, Clone, Copy)]
struct Sample {
    x: f64,
    y: f64,
    z: f64,
}

/// One robust sky sample per tile — the tile centre with coordinates normalized to `[-1, 1]` and
/// the shared [`crate::background_mesh::TileGrid`] SExtractor-style sky estimate (per-tile ±σ-clip
/// → Pearson mode). Reuses the exact estimator star detection uses, so the gradient fit and the
/// detector see the same sky. The grid 3×3 median filter is **off** (it would bias a real
/// gradient's boundary tiles; outlier tiles are instead rejected by the surface fit's residual
/// clip). A `None` object mask for now — a star/bright-signal mask from the star detector belongs
/// in that slot.
fn collect_samples(
    channel: &Buffer2<f32>,
    tile: usize,
    workspace: &mut MeshWorkspace,
) -> Vec<Sample> {
    let size = Size2us::new(channel.width(), channel.height());
    let grid = workspace.tile_stats(channel, None, tile, DEFAULT_SIGMA_CLIP_ITERATIONS, false);

    let mut samples = Vec::with_capacity(grid.stats.width() * grid.stats.height());
    for ty in 0..grid.stats.height() {
        let y = norm(f64::from(grid.centers_y[ty]), size.height);
        for (tx, &cx) in grid.centers_x.iter().enumerate() {
            samples.push(Sample {
                x: norm(f64::from(cx), size.width),
                y,
                z: f64::from(grid.stats[(tx, ty)].sky),
            });
        }
    }
    samples
}

/// Largest degree whose term count `(d+1)(d+2)/2` fits within `n` samples (so the fit is
/// determined).
fn effective_degree(n: usize, requested: usize) -> usize {
    let mut d = requested.min(4);
    while d > 0 && (d + 1) * (d + 2) / 2 > n {
        d -= 1;
    }
    d
}

/// Exponent pairs `(i, j)` for every monomial `x^i·y^j` with `i + j ≤ degree`.
fn poly_terms(degree: usize) -> Vec<(u8, u8)> {
    let mut terms = Vec::new();
    for total in 0..=u8::try_from(degree).expect("the degree is at most 4") {
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
fn eval(coeffs: &DVector<f64>, terms: &[(u8, u8)], x: f64, y: f64) -> f64 {
    terms
        .iter()
        .zip(coeffs.iter())
        .map(|(&(i, j), &c)| c * x.powi(i32::from(i)) * y.powi(i32::from(j)))
        .sum()
}

/// Least-squares solve of the original design matrix using SVD.
fn solve_ls(samples: &[Sample], terms: &[(u8, u8)]) -> Result<DVector<f64>, OpError> {
    let (m, k) = (samples.len(), terms.len());
    let a = DMatrix::from_fn(m, k, |r, c| {
        let (i, j) = terms[c];
        samples[r].x.powi(i32::from(i)) * samples[r].y.powi(i32::from(j))
    });
    let z = DVector::from_fn(m, |r, _| samples[r].z);
    let svd = a.svd(true, true);
    let largest_singular_value = svd.singular_values.iter().copied().fold(0.0, f64::max);
    let tolerance = f64::EPSILON * m.max(k) as f64 * largest_singular_value;
    let rank = svd.rank(tolerance);
    if rank < k {
        return Err(OpError::RankDeficient {
            operation: "background surface fit",
            rank,
            required_rank: k,
        });
    }
    Ok(svd
        .solve(&z, tolerance)
        .expect("SVD was constructed with both singular-vector matrices"))
}

/// Fit the surface, then iteratively reject samples whose residual exceeds `kappa·σ` and refit
/// (σ = MAD-scaled residual spread). Rejects tiles sitting on nebulosity or unrejected stars.
fn fit_surface(
    samples: &[Sample],
    terms: &[(u8, u8)],
    kappa: f32,
    iterations: usize,
) -> Result<DVector<f64>, OpError> {
    let mut active: Vec<Sample> = samples.to_vec();
    let mut coeffs = solve_ls(&active, terms)?;
    // Reused by every refit rather than reallocated per iteration.
    let mut deviations: Vec<f64> = Vec::new();
    for _ in 0..iterations {
        let residuals: Vec<f64> = active
            .iter()
            .map(|s| s.z - eval(&coeffs, terms, s.x, s.y))
            .collect();
        let sigma = robust_sigma_f64(&residuals, &mut deviations);
        if sigma <= 0.0 {
            break;
        }
        let thresh = f64::from(kappa) * sigma;
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
        coeffs = solve_ls(&active, terms)?;
    }
    Ok(coeffs)
}

/// The fitted polynomial surface, packed for fast on-the-fly evaluation.
///
/// Rather than re-evaluate the bivariate polynomial per pixel (a `powi` per term), the coefficients
/// are packed into a `(degree+1)²` matrix `C[i][j]`. For each row `y` the powers `y^j` collapse `C`
/// into a 1-D polynomial in `x` (`b[i] = Σ_j C[i][j]·y^j`), which every pixel in the row evaluates
/// by Horner — `degree` fused multiply-adds, no `powi`, and no full-resolution model plane.
#[derive(Debug)]
struct Surface {
    /// Row-major `C[i*d1 + j]` for the monomials `x^i·y^j`; `(degree+1)² ≤ 25`.
    c_mat: [f64; 25],
    degree: usize,
    size: Size2us,
}

impl Surface {
    fn new(coeffs: &DVector<f64>, terms: &[(u8, u8)], size: Size2us) -> Self {
        let degree = terms
            .iter()
            .map(|&(i, j)| usize::from(i + j))
            .max()
            .unwrap_or(0);
        // `effective_degree` caps the surface at 4, so `degree + 1 ≤ 5` fits fixed buffers.
        assert!(degree <= 4, "background surface degree {degree} exceeds 4");
        let d1 = degree + 1;
        let mut c_mat = [0.0f64; 25];
        for (&(i, j), &c) in terms.iter().zip(coeffs.iter()) {
            c_mat[usize::from(i) * d1 + usize::from(j)] = c;
        }
        Self {
            c_mat,
            degree,
            size,
        }
    }

    /// Collapse the y dimension for row `y`: `b[i] = Σ_j C[i][j]·y^j`.
    fn row_coeffs(&self, y: usize) -> [f64; 5] {
        let d1 = self.degree + 1;
        let ny = norm(y as f64, self.size.height);
        let mut yp = [0.0f64; 5];
        yp[0] = 1.0;
        for j in 1..d1 {
            yp[j] = yp[j - 1] * ny;
        }
        let mut b = [0.0f64; 5];
        for (i, b_i) in b[..d1].iter_mut().enumerate() {
            *b_i = (0..d1).map(|j| self.c_mat[i * d1 + j] * yp[j]).sum();
        }
        b
    }

    /// Rewrite every pixel as `remove(pixel, model)`, evaluating the surface on the fly,
    /// parallel over rows.
    fn remove(&self, plane: &mut Buffer2<f32>, remove: impl Fn(f32, f32) -> f32 + Sync) {
        let (w, degree) = (self.size.width, self.degree);
        plane
            .pixels_mut()
            .par_chunks_mut(w)
            .enumerate()
            .for_each(|(y, row)| {
                let b = self.row_coeffs(y);
                for (x, p) in row.iter_mut().enumerate() {
                    let nx = norm(x as f64, w);
                    let mut acc = b[degree];
                    for i in (0..degree).rev() {
                        acc = acc * nx + b[i];
                    }
                    *p = remove(*p, acc as f32);
                }
            });
    }

    /// Mean of the surface over the pixel grid, exact in `O(w + h)`: the monomials are
    /// separable, so `mean = Σ C[i][j] · mean_x(x^i) · mean_y(y^j)`.
    fn mean(&self) -> f64 {
        let d1 = self.degree + 1;
        let mx = axis_moments(self.size.width, d1);
        let my = axis_moments(self.size.height, d1);
        let mut mean = 0.0;
        for (i, &mx_i) in mx[..d1].iter().enumerate() {
            for (j, &my_j) in my[..d1].iter().enumerate() {
                mean += self.c_mat[i * d1 + j] * mx_i * my_j;
            }
        }
        mean
    }
}

/// Per-axis moments `mean(t^i)` for `i < d1` of the normalized coordinate over an
/// `n`-pixel axis.
fn axis_moments(n: usize, d1: usize) -> [f64; 5] {
    let mut moments = [0.0f64; 5];
    for k in 0..n {
        let t = norm(k as f64, n);
        let mut p = 1.0;
        for moment in &mut moments[..d1] {
            *moment += p;
            p *= t;
        }
    }
    for moment in &mut moments {
        *moment /= n as f64;
    }
    moments
}

#[cfg(test)]
mod tests;
