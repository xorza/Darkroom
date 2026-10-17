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
//! pollution is colored), and runs on the linear master *before* color calibration and the
//! stretch.

use arrayvec::ArrayVec;
use common::{Introspect, IntrospectEnum};
use imaginarium::Buffer2;
use nalgebra::{DMatrix, DVector};
use rayon::prelude::*;

use crate::background_mesh::mesh_workspace::{
    DEFAULT_SIGMA_CLIP_MAX_ITERATIONS, MedianFilter, MeshParams, MeshWorkspace,
};
use crate::config_value::ConfigValue;
use crate::error::InvalidConfigField;
use crate::image::linear_image::LinearImage;
use crate::image_ops::error::OpError;
use crate::math::lstsq::Lstsq;
use crate::math::size2us::Size2us;
use crate::math::statistics::median_mad::MedianMad;

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
    /// Every channel's model is fitted before any is removed, so a channel that fails leaves the
    /// image as it was.
    ///
    /// # Errors
    /// [`OpError::Config`] on out-of-range parameters; [`OpError::TooFewSamples`] when the image
    /// gives fewer sample tiles than the requested polynomial has terms;
    /// [`OpError::RankDeficient`] when the sample geometry cannot determine it;
    /// [`OpError::NonPositiveBackground`] when [`BackgroundMode::Divide`] meets a model whose mean
    /// is not positive.
    pub fn apply(&self, image: &mut LinearImage) -> Result<(), OpError> {
        self.validate()?;
        let mut workspace = MeshWorkspace::default();
        let surfaces = (0..image.channels())
            .map(|channel| self.fit_plane(image.channel(channel), &mut workspace))
            .collect::<Result<ArrayVec<Surface, 3>, OpError>>()?;
        for (plane, surface) in image.planes_mut().zip(&surfaces) {
            self.remove_from(plane, surface);
        }
        Ok(())
    }

    /// Fit one channel's background surface. A divide needs a positive model mean to normalize
    /// the model by, and anything else would leave the channel silently unchanged.
    fn fit_plane(
        &self,
        plane: &Buffer2<f32>,
        workspace: &mut MeshWorkspace,
    ) -> Result<Surface, OpError> {
        let samples = collect_samples(plane, self.tile_size, workspace);
        let terms = Monomial::up_to(self.degree);
        if terms.len() > samples.len() {
            return Err(OpError::TooFewSamples {
                samples: samples.len(),
                degree: self.degree,
            });
        }
        let polynomial = self.fit_surface(&samples, &terms)?;
        let surface = Surface::new(&polynomial, Size2us::from(plane));
        let mean = surface.mean();
        if self.mode == BackgroundMode::Divide && mean <= 0.0 {
            return Err(OpError::NonPositiveBackground { mean });
        }
        Ok(surface)
    }

    /// Fit the surface, then iteratively reject samples whose residual exceeds
    /// `rejection_sigma·σ` and refit (σ = MAD-scaled residual spread). Rejects tiles sitting on
    /// nebulosity or unrejected stars.
    fn fit_surface<'a>(
        &self,
        samples: &[Sample],
        terms: &'a [Monomial],
    ) -> Result<Polynomial<'a>, OpError> {
        let mut active: Vec<Sample> = samples.to_vec();
        let mut polynomial = Polynomial::fit(terms, &active)?;
        // Reused by every refit rather than reallocated per iteration.
        let mut scratch: Vec<f64> = Vec::new();
        for _ in 0..self.iterations {
            let residuals: Vec<f64> = active
                .iter()
                .map(|s| s.z - polynomial.at(s.x, s.y))
                .collect();
            scratch.clear();
            scratch.extend_from_slice(&residuals);
            let sigma = MedianMad::of_mut(&mut scratch).sigma();
            if sigma <= 0.0 {
                break;
            }
            let thresh = f64::from(self.rejection_sigma) * sigma;
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
            polynomial = Polynomial::fit(terms, &active)?;
        }
        Ok(polynomial)
    }

    /// Remove a fitted surface from its channel, in place. The model is evaluated on the fly inside
    /// the removal pass ([`Surface::remove`]) — a degree ≤ 4 polynomial needs no full-resolution
    /// model plane.
    fn remove_from(&self, plane: &mut Buffer2<f32>, surface: &Surface) {
        let mean = surface.mean() as f32;
        match self.mode {
            BackgroundMode::Subtract => {
                // Only the model's variation is removed; its mean stays as the sky pedestal, as
                // Siril does. Without it the sky sits at ≈0 and the next step that measures the
                // background against zero (an auto stretch) has nothing to place. Each channel
                // keeps its own level, like `Divide` — neutralizing the sky color is a separate
                // op.
                surface.remove(plane, |p, m| p - (m - mean));
            }
            BackgroundMode::Divide => {
                let floor = self.divide_floor;
                surface.remove(plane, |p, m| p / (m / mean).max(floor));
            }
        }
    }

    fn validate(&self) -> Result<(), InvalidConfigField> {
        ConfigValue::count("background extraction degree", self.degree)
            .must_be("between 1 and 4", (1..=4).contains(&self.degree))?;
        ConfigValue::count("background extraction tile_size", self.tile_size)
            .must_be("at least 8", self.tile_size >= 8)?;
        ConfigValue::new(
            "background extraction rejection_sigma",
            self.rejection_sigma,
        )
        .finite_and("finite and positive", |value| value > 0.0)?;
        ConfigValue::new("background extraction divide_floor", self.divide_floor)
            .finite_and("finite and in (0, 1]", |value| value > 0.0 && value <= 1.0)
    }
}

/// One robust sky sample per tile, at the tile centre, with coordinates normalized to `[-1, 1]`.
#[derive(Debug, Clone, Copy)]
struct Sample {
    x: f64,
    y: f64,
    z: f64,
}

/// One robust sky sample per tile — the tile centre with coordinates normalized to `[-1, 1]` and
/// the shared [`crate::background_mesh::BackgroundMesh`] SExtractor-style sky estimate (per-tile
/// ±σ-clip → Pearson mode), the estimator star detection reads its sky from. Its grid 3×3 median
/// filter is **off** here, where the detector's is on: it would bias a real gradient's boundary
/// tiles, and outlier tiles are instead rejected by the surface fit's residual clip. A `None`
/// object mask for now — a star/bright-signal mask from the star detector belongs in that slot.
fn collect_samples(
    channel: &Buffer2<f32>,
    tile: usize,
    workspace: &mut MeshWorkspace,
) -> Vec<Sample> {
    let size = Size2us::from(channel);
    let grid = workspace.measure(
        channel,
        None,
        None,
        MeshParams {
            tile_size: tile,
            sigma_clip_max_iterations: DEFAULT_SIGMA_CLIP_MAX_ITERATIONS,
            median_filter: MedianFilter::Skip,
        },
    );

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

/// One monomial of the surface, by the exponent of each coordinate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Monomial {
    x: u8,
    y: u8,
}

impl Monomial {
    /// Every monomial `x^i·y^j` with `i + j ≤ degree`, by total degree, then by `i`.
    fn up_to(degree: usize) -> Vec<Self> {
        let mut terms = Vec::new();
        for total in 0..=u8::try_from(degree).expect("the degree is at most 4") {
            for x in 0..=total {
                terms.push(Self { x, y: total - x });
            }
        }
        terms
    }
}

/// A polynomial in the normalized coordinates: its monomials and their coefficients.
#[derive(Debug)]
struct Polynomial<'a> {
    terms: &'a [Monomial],
    coeffs: DVector<f64>,
}

impl<'a> Polynomial<'a> {
    /// The least-squares fit of `terms` to `samples`, by the SVD of the design matrix.
    fn fit(terms: &'a [Monomial], samples: &[Sample]) -> Result<Self, OpError> {
        let (m, k) = (samples.len(), terms.len());
        let a = DMatrix::from_fn(m, k, |r, c| {
            let term = terms[c];
            samples[r].x.powi(i32::from(term.x)) * samples[r].y.powi(i32::from(term.y))
        });
        let z = DMatrix::from_fn(m, 1, |r, _| samples[r].z);
        let lstsq = Lstsq::new(a);
        let solution = lstsq.solve(&z).ok_or_else(|| OpError::RankDeficient {
            rank: lstsq.rank(),
            required_rank: k,
        })?;
        Ok(Self {
            terms,
            coeffs: DVector::from_column_slice(solution.as_slice()),
        })
    }

    /// The polynomial at normalized `(x, y)`.
    fn at(&self, x: f64, y: f64) -> f64 {
        self.terms
            .iter()
            .zip(self.coeffs.iter())
            .map(|(term, &c)| c * x.powi(i32::from(term.x)) * y.powi(i32::from(term.y)))
            .sum()
    }
}

/// Map a pixel/centre coordinate to `[-1, 1]` (conditioning for the least-squares fit).
fn norm(c: f64, n: usize) -> f64 {
    if n > 1 {
        2.0 * c / (n as f64 - 1.0) - 1.0
    } else {
        0.0
    }
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
    fn new(polynomial: &Polynomial<'_>, size: Size2us) -> Self {
        let degree = polynomial
            .terms
            .iter()
            .map(|term| usize::from(term.x + term.y))
            .max()
            .unwrap_or(0);
        // `validate` caps the degree at 4, so `degree + 1 ≤ 5` fits fixed buffers.
        debug_assert!(degree <= 4, "background surface degree {degree} exceeds 4");
        let d1 = degree + 1;
        let mut c_mat = [0.0f64; 25];
        for (term, &c) in polynomial.terms.iter().zip(polynomial.coeffs.iter()) {
            c_mat[usize::from(term.x) * d1 + usize::from(term.y)] = c;
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
