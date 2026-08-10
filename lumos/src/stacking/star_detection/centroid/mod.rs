//! Sub-pixel centroid computation and star quality metrics.
//!
//! Uses iterative weighted centroid algorithm for sub-pixel accurate positioning,
//! typically achieving ~0.05 pixel accuracy.
//!
//! Also provides 2D Gaussian and Moffat profile fitting for higher precision
//! centroid computation (~0.01 pixel accuracy).
//!
//! Positions are f64 end to end. Every accumulator here is already f64, [`Star::pos`] is a
//! [`DVec2`], and registration solves its transforms in f64 — an f32 carrier would only add a
//! narrowing in the middle. It would also quantize coarser than
//! [`CENTROID_CONVERGENCE_THRESHOLD`] beyond x ≈ 1024, which silently turns the moments loop's
//! convergence test into an exact-equality check on the outer parts of a large frame.

mod gaussian_fit;
mod linear_solver;
mod lm_optimizer;
mod moffat_fit;
#[cfg(any(target_arch = "x86_64", target_arch = "aarch64"))]
mod simd;

#[cfg(all(test, feature = "internals"))]
mod bench;
#[cfg(test)]
mod internals;
#[cfg(test)]
mod tests;

use arrayvec::ArrayVec;
use glam::DVec2;

use crate::math::fwhm::{FWHM_TO_SIGMA, sigma_to_fwhm};
use crate::math::size2us::Size2us;
use crate::math::statistics::ClippedStats;
use crate::stacking::star_detection::background::background_estimate::BackgroundEstimate;
use crate::stacking::star_detection::config::measurement_config::{
    CentroidMethod, LocalBackgroundMethod, MeasurementConfig, NoiseModel,
};
use crate::stacking::star_detection::deblend::region::Region;
use crate::stacking::star_detection::roundness::Roundness;
use crate::stacking::star_detection::star::Star;
use gaussian_fit::{GaussianFit, GaussianFitConfig};
use imaginarium::Buffer2;
use lm_optimizer::FitData;
use moffat_fit::{MoffatFit, MoffatFitConfig};

/// Stamp radius as a multiple of FWHM.
///
/// A stamp radius of 1.75 × FWHM captures approximately 99% of the PSF flux
/// for a Gaussian profile, providing accurate centroid and flux measurements
/// while minimizing background contamination.
const STAMP_RADIUS_FWHM_FACTOR: f32 = 1.75;

/// Minimum stamp radius in pixels.
///
/// Ensures sufficient pixels for accurate centroid computation even for
/// very small PSFs or undersampled images.
const MIN_STAMP_RADIUS: usize = 4;

/// Maximum stamp radius in pixels.
///
/// Limits computation time and prevents excessive background inclusion
/// for very large PSFs.
const MAX_STAMP_RADIUS: usize = 15;

/// Maximum stamp side length in pixels (31 for stamp_radius=15).
const MAX_STAMP_SIZE: usize = 2 * MAX_STAMP_RADIUS + 1;

/// Maximum stamp pixels (31×31 for stamp_radius=15).
const MAX_STAMP_PIXELS: usize = MAX_STAMP_SIZE.pow(2);

/// Maximum annulus outer radius (1.5 × MAX_STAMP_RADIUS, rounded up).
const MAX_ANNULUS_OUTER_RADIUS: usize = (MAX_STAMP_RADIUS * 3).div_ceil(2); // = 23

/// Maximum annulus pixels for LocalAnnulus background method.
/// Computed as the area of a square with side 2×outer_radius+1.
const MAX_ANNULUS_PIXELS: usize = (2 * MAX_ANNULUS_OUTER_RADIUS + 1).pow(2); // = 47² = 2209

/// Centroid convergence threshold in pixels.
///
/// Iteration stops when the distance moved is less than this value.
/// Set to 0.0001 (0.1 millipixel) for sub-pixel astrometric precision.
const CENTROID_CONVERGENCE_THRESHOLD: f64 = 0.0001;

/// Maximum weighted-moments iterations for standalone centroid (no fitting follows).
const MAX_MOMENTS_ITERATIONS: usize = 10;

/// Weighted-moments iterations when L-M fitting follows.
/// Only needs to provide a rough seed — L-M refines position independently.
const MOMENTS_ITERATIONS_BEFORE_FIT: usize = 2;

/// Convergence threshold in pixels squared.
const CONVERGENCE_THRESHOLD_SQ: f64 =
    CENTROID_CONVERGENCE_THRESHOLD * CENTROID_CONVERGENCE_THRESHOLD;

/// Compute stamp radius from expected FWHM.
#[inline]
pub(super) fn compute_stamp_radius(expected_fwhm: f32) -> usize {
    let radius = (expected_fwhm * STAMP_RADIUS_FWHM_FACTOR).ceil() as usize;
    radius.clamp(MIN_STAMP_RADIUS, MAX_STAMP_RADIUS)
}

/// Check if position is within valid bounds for stamp extraction.
#[inline]
fn is_valid_stamp_position(pos: DVec2, size: Size2us, stamp_radius: usize) -> bool {
    let icx = pos.x.round() as isize;
    let icy = pos.y.round() as isize;
    icx >= stamp_radius as isize
        && icy >= stamp_radius as isize
        && icx < (size.width - stamp_radius) as isize
        && icy < (size.height - stamp_radius) as isize
}

/// The stamp's own pixel coordinates, `0..2r` on each axis, flattened row-major.
///
/// Identical for every candidate of a given radius, so it is built once per detection rather than
/// per star: both profile models read the data only through `x - x0` and `y - y0`, so fitting in
/// stamp-local coordinates and shifting the centre back afterwards is the same fit. The smaller
/// magnitudes also condition the normal equations a little better than image coordinates do.
#[derive(Debug)]
pub(super) struct StampGrid {
    x: ArrayVec<f64, MAX_STAMP_PIXELS>,
    y: ArrayVec<f64, MAX_STAMP_PIXELS>,
    radius: usize,
}

impl StampGrid {
    pub(super) fn new(radius: usize) -> Self {
        let side = 2 * radius + 1;
        let mut x = ArrayVec::new();
        let mut y = ArrayVec::new();
        for row in 0..side {
            for column in 0..side {
                x.push(column as f64);
                y.push(row as f64);
            }
        }
        Self { x, y, radius }
    }
}

/// Stack-allocated stamp data extracted around a star candidate.
/// Uses ArrayVec to avoid heap allocations for typical stamp sizes.
#[derive(Debug)]
struct StampData {
    /// Pixel values (background-subtracted at the caller if needed), row-major over the stamp.
    /// The matching coordinates live in the shared [`StampGrid`].
    z: ArrayVec<f64, MAX_STAMP_PIXELS>,
    /// Peak pixel value within the stamp.
    peak: f32,
    /// Image position of the stamp's top-left pixel, which [`StampGrid`]'s coordinates are
    /// relative to. Integer-valued, but f64 so shifting a fitted centre back into image
    /// coordinates costs no rounding.
    origin: DVec2,
}

/// Noise inputs for an inverse-variance-weighted fit: the local sky σ plus the
/// normalized-domain sensor model. `None` (absent) means an unweighted fit.
#[derive(Debug, Clone, Copy)]
struct FitNoise {
    sky_noise: f32,
    noise_model: NoiseModel,
}

impl FitNoise {
    /// Inverse-variance weight for one pixel, using the CCD noise model (the same per-pixel
    /// decomposition as `compute_snr`):
    /// `w = 1 / (signal/G + sky_noise² + (read_noise_electrons/G)²)`, where `G` is electrons per
    /// normalized unit.
    ///
    /// Down-weights the shot-noisy bright core so the fit is the ML estimator instead of
    /// over-weighting high-signal pixels (which biases the sub-pixel centroid/FWHM/flux). The
    /// variance is floored so a zero-variance pixel cannot produce an infinite weight.
    #[inline]
    fn weight(&self, z: f64, background: f64) -> f64 {
        let signal = (z - background).max(0.0);
        1.0 / self
            .noise_model
            .variance_normalized(signal, self.sky_noise as f64, 1)
            .max(1e-12)
    }
}

/// Gaussian-equivalent width from a stamp's weighted second moments: for a Gaussian
/// `E[r²] = 2σ²`, so `σ = sqrt(E[r²]/2)`. A better seed for L-M than a fixed value.
///
/// `max_sigma` is the widest profile the optimizer will accept, so the seed lands inside the
/// range its own `constrain` enforces — starting outside it just spends the first iteration
/// being clamped back. Always at least [`MIN_STAMP_RADIUS`], so the 0.5 floor cannot cross it.
fn sigma_from_moments(sum_r2: f64, sum_w: f64, max_sigma: f64) -> f32 {
    if sum_w > f64::EPSILON {
        (sum_r2 / sum_w / 2.0).sqrt().clamp(0.5, max_sigma) as f32
    } else {
        2.0
    }
}

/// The per-candidate inputs both profile fits need before the optimizer runs: the stamp, the
/// centre expressed in the stamp's own frame, the inverse-variance weights, and a width seed.
///
/// The two fits differ only in the model they hand to the optimizer and the parameters they read
/// back out; everything up to that point is this.
#[derive(Debug)]
struct StampFit {
    stamp: StampData,
    /// `pos` relative to [`StampData::origin`] — the frame the fit runs in.
    local_pos: DVec2,
    weights: Option<ArrayVec<f64, MAX_STAMP_PIXELS>>,
    /// Gaussian-equivalent width from the stamp's second moments, seeding the optimizer.
    sigma_est: f32,
}

impl StampFit {
    /// `None` when the stamp falls outside the frame, or holds too few pixels to constrain `N`
    /// free parameters — a least-squares fit needs strictly more samples than parameters.
    ///
    /// One pass produces all four outputs. Pixel values, the peak, the second moments behind
    /// `sigma_est` and the inverse-variance weights used to be an extraction walk plus one walk per
    /// consumer — three traversals of the same 225 f64 per candidate, on the hottest path in
    /// `measure_star`.
    fn prepare<const N: usize>(
        pixels: &Buffer2<f32>,
        pos: DVec2,
        grid: &StampGrid,
        background: f32,
        noise: Option<FitNoise>,
    ) -> Option<Self> {
        let radius = grid.radius;
        if !is_valid_stamp_position(pos, Size2us::new(pixels.width(), pixels.height()), radius) {
            return None;
        }
        let stamp_size = 2 * radius + 1;
        if stamp_size * stamp_size <= N {
            return None;
        }

        let icx = pos.x.round() as isize;
        let icy = pos.y.round() as isize;
        // Moment arms measured from the rounded centre: at column `dx` the offset from the true
        // centre is `dx - frac_x`, which is the shared grid's coordinate minus `local_pos` without
        // going through either array.
        let frac_x = pos.x - icx as f64;
        let frac_y = pos.y - icy as f64;
        let sky = background as f64;

        let mut z = ArrayVec::new();
        // Filled only for a weighted fit, but an empty `ArrayVec` costs nothing to carry: its
        // backing storage is uninitialized until pushed to.
        let mut weights = ArrayVec::new();
        let mut peak = f32::MIN;
        let mut sum_r2 = 0.0f64;
        let mut sum_w = 0.0f64;

        let radius_i32 = radius as i32;
        for dy in -radius_i32..=radius_i32 {
            let y = (icy + dy as isize) as usize;
            // One bounds check per row rather than per pixel — the guard above has already
            // established the whole stamp is inside the frame.
            let row = pixels.row(y);
            let ddy = dy as f64 - frac_y;

            for dx in -radius_i32..=radius_i32 {
                let value = row[(icx + dx as isize) as usize];
                let value64 = f64::from(value);
                z.push(value64);
                peak = peak.max(value);

                let signal = (value64 - sky).max(0.0);
                let ddx = dx as f64 - frac_x;
                sum_r2 += signal * (ddx * ddx + ddy * ddy);
                sum_w += signal;

                if let Some(n) = noise {
                    weights.push(n.weight(value64, sky));
                }
            }
        }

        let origin = DVec2::new(
            (icx - radius as isize) as f64,
            (icy - radius as isize) as f64,
        );
        Some(Self {
            stamp: StampData { z, peak, origin },
            // Fit in the stamp's own frame: the models are translation-invariant, so this is the
            // same fit with better-conditioned magnitudes, and the coordinate arrays become the
            // shared grid instead of two per-candidate ramps.
            local_pos: pos - origin,
            weights: noise.map(|_| weights),
            sigma_est: sigma_from_moments(sum_r2, sum_w, radius as f64),
        })
    }

    /// The optimizer's view of the stamp: coordinate ramps shared across the whole detection,
    /// pixel values and weights owned per candidate.
    fn data<'a>(&'a self, grid: &'a StampGrid) -> FitData<'a> {
        FitData::new(&grid.x, &grid.y, &self.stamp.z, self.weights.as_deref())
    }

    /// Amplitude seed: the stamp's peak above the sky, floored so the optimizer starts positive.
    fn amplitude_seed(&self, background: f32) -> f64 {
        (self.stamp.peak - background).max(0.01) as f64
    }

    /// Lift a fitted centre out of the stamp frame back into image coordinates.
    fn to_image(&self, x0: f64, y0: f64) -> DVec2 {
        DVec2::new(x0, y0) + self.stamp.origin
    }
}

/// Flat per-stamp sky estimate: one (background, noise) pair valid at the stamp
/// scale, as opposed to the per-pixel tiled global map.
#[derive(Debug, Clone, Copy)]
struct LocalBackground {
    bg: f32,
    noise: f32,
}

/// Compute local background and noise using an annular region around the star.
///
/// The inner radius excludes the star's flux, and the outer radius samples
/// the local sky. Uses sigma-clipped median for robustness.
///
/// # Arguments
/// * `pixels` - Image data
/// * `width` - Image width
/// * `height` - Image height
/// * `pos` - Star center position
/// * `inner_radius` - Inner radius of annulus (excludes star)
/// * `outer_radius` - Outer radius of annulus
///
/// # Returns
/// The local background/noise, or None if not enough valid pixels
fn compute_annulus_background(
    pixels: &Buffer2<f32>,
    pos: DVec2,
    inner_radius: usize,
    outer_radius: usize,
) -> Option<LocalBackground> {
    let icx = pos.x.round() as isize;
    let icy = pos.y.round() as isize;
    let inner_r2 = (inner_radius * inner_radius) as f32;
    let outer_r2 = (outer_radius * outer_radius) as f32;

    // Use stack-allocated ArrayVec to avoid heap allocation
    let mut values: ArrayVec<f32, MAX_ANNULUS_PIXELS> = ArrayVec::new();

    let width = pixels.width() as isize;
    let height = pixels.height() as isize;
    let outer_r_i32 = outer_radius as i32;
    for dy in -outer_r_i32..=outer_r_i32 {
        // Row bound first so the row slice — and its bounds check — is taken once, not per
        // column. The annulus can hang off the frame, so the row may not exist at all.
        let y = icy + dy as isize;
        if y < 0 || y >= height {
            continue;
        }
        let row = pixels.row(y as usize);

        for dx in -outer_r_i32..=outer_r_i32 {
            let r2 = (dx * dx + dy * dy) as f32;
            if r2 < inner_r2 || r2 > outer_r2 {
                continue;
            }

            let x = icx + dx as isize;
            if x >= 0 && x < width {
                values.push(row[x as usize]);
            }
        }
    }

    if values.len() < 10 {
        return None;
    }

    // Sigma-clipped median (2 iterations, 3-sigma clip)
    let stats = sigma_clipped_median_mad(&mut values, 3.0, 2);
    Some(LocalBackground {
        bg: stats.median,
        noise: stats.sigma,
    })
}

/// Compute sigma-clipped median and MAD using the shared implementation.
/// Uses stack-allocated ArrayVec for deviations to avoid heap allocation.
#[inline]
fn sigma_clipped_median_mad(values: &mut [f32], kappa: f32, iterations: usize) -> ClippedStats {
    // Stack scratch: this runs per star inside the parallel measure loop, so it must not allocate.
    let mut deviations: ArrayVec<f32, MAX_ANNULUS_PIXELS> = ArrayVec::new();
    ClippedStats::sigma_clipped(values, &mut deviations, kappa, iterations)
}

/// Measure a star candidate: compute sub-pixel position and quality metrics.
///
/// This is the main entry point for the measurement stage. It takes a detected
/// region and computes:
/// - Sub-pixel position using the configured centroid method
/// - Quality metrics: flux, FWHM, eccentricity, SNR, sharpness, roundness
/// - Laplacian SNR for cosmic ray detection
///
/// Returns `None` if the candidate fails quality checks during measurement.
///
/// # Centroid Methods
///
/// The position refinement method is selected via `config.centroid_method`:
/// - `WeightedMoments`: Iterative weighted centroid (~0.05 pixel accuracy, fast)
/// - `GaussianFit`: 2D Gaussian fitting (~0.01 pixel accuracy, slower)
/// - `MoffatFit`: 2D Moffat fitting (~0.01 pixel accuracy, best for atmospheric seeing)
pub(super) fn measure_star(
    pixels: &Buffer2<f32>,
    background: &BackgroundEstimate,
    region: &Region,
    config: &MeasurementConfig,
    expected_fwhm: f32,
    grid: &StampGrid,
) -> Option<Star> {
    // Built once per detection by the measure stage, from this same `expected_fwhm`.
    let stamp_radius = grid.radius;
    debug_assert_eq!(stamp_radius, compute_stamp_radius(expected_fwhm));

    // Initial position from peak
    let mut pos = DVec2::new(region.peak.x as f64, region.peak.y as f64);

    // First pass: weighted moments for initial refinement.
    // When a fitting method follows, only 2 iterations are needed — the L-M
    // optimizer refines position independently and converges to the same result
    // regardless of Phase 1 precision (verified by tests).
    let phase1_iters = match config.centroid_method {
        CentroidMethod::WeightedMoments => MAX_MOMENTS_ITERATIONS,
        CentroidMethod::GaussianFit | CentroidMethod::MoffatFit { .. } => {
            MOMENTS_ITERATIONS_BEFORE_FIT
        }
    };
    for _ in 0..phase1_iters {
        let new_pos = refine_centroid(pixels, background, pos, stamp_radius, expected_fwhm)?;

        let delta = new_pos - pos;
        pos = new_pos;

        if delta.length_squared() < CONVERGENCE_THRESHOLD_SQ {
            break;
        }
    }

    // Compute local background based on configured method
    let icx = pos.x.round() as isize;
    let icy = pos.y.round() as isize;
    let bg_y = icy as usize;
    let bg_x = icx as usize;
    let global_fallback = || LocalBackground {
        bg: background.background.row(bg_y)[bg_x],
        noise: background.noise.row(bg_y)[bg_x],
    };

    // The annulus estimate; None in GlobalMap mode or when the annulus has too few
    // in-bounds samples (star near an edge). A failed annulus falls back to the
    // center pixel of the global map for the fit seed below, but must NOT become a
    // metrics override: flattening one map pixel across the whole stamp would be
    // strictly worse than the per-pixel map itself.
    let annulus_at = |at: DVec2| match config.local_background {
        LocalBackgroundMethod::GlobalMap => None,
        LocalBackgroundMethod::LocalAnnulus => {
            let inner_radius = stamp_radius;
            let outer_radius = (stamp_radius as f32 * 1.5).ceil() as usize;
            compute_annulus_background(pixels, at, inner_radius, outer_radius)
        }
    };
    let moments_pos = pos;
    let annulus_background = annulus_at(pos);
    let LocalBackground {
        bg: local_bg,
        noise: local_noise,
    } = annulus_background.unwrap_or_else(global_fallback);

    // Refine with profile fitting if requested.
    // When fit converges, also extract FWHM and eccentricity from fit parameters
    // (more accurate than moment-based estimates).
    let mut fit_fwhm: Option<f32> = None;
    let mut fit_eccentricity: Option<f32> = None;

    // Inverse-variance fit weights when a noise model is configured (PR1).
    let fit_noise = config.noise_model.map(|noise_model| FitNoise {
        sky_noise: local_noise,
        noise_model,
    });

    match config.centroid_method {
        CentroidMethod::GaussianFit => {
            let fit_config = GaussianFitConfig {
                position_convergence_threshold: CENTROID_CONVERGENCE_THRESHOLD,
                ..GaussianFitConfig::default()
            };
            let fit = GaussianFit::new(pixels, pos, grid, local_bg, fit_noise, &fit_config);
            if let Some(result) = fit.filter(|r| r.converged) {
                pos = result.pos;
                // FWHM from geometric mean of sigma_x, sigma_y
                let geo_sigma = (result.sigma.x * result.sigma.y).sqrt();
                fit_fwhm = Some(sigma_to_fwhm(geo_sigma));
                // Eccentricity from sigma ratio: e = sqrt(1 - (min/max)^2)
                let (s_min, s_max) = if result.sigma.x < result.sigma.y {
                    (result.sigma.x, result.sigma.y)
                } else {
                    (result.sigma.y, result.sigma.x)
                };
                if s_max > f32::EPSILON {
                    let ratio = s_min / s_max;
                    fit_eccentricity = Some((1.0 - ratio * ratio).sqrt().clamp(0.0, 1.0));
                }
            }
        }
        CentroidMethod::MoffatFit { beta } => {
            let fit_config = MoffatFitConfig {
                fixed_beta: beta,
                lm: lm_optimizer::LMConfig {
                    position_convergence_threshold: CENTROID_CONVERGENCE_THRESHOLD,
                    ..lm_optimizer::LMConfig::default()
                },
            };
            let fit = MoffatFit::new(pixels, pos, grid, local_bg, fit_noise, &fit_config);
            if let Some(result) = fit.filter(|r| r.converged) {
                pos = result.pos;
                fit_fwhm = Some(result.fwhm);
                // Moffat is radially symmetric (single alpha) — eccentricity stays moment-based
            }
        }
        CentroidMethod::WeightedMoments => {
            // Already computed above
        }
    };

    // The estimate above was centred on the moments position, and the fit has since moved the
    // star. `compute_annulus_background` samples by rounded centre, so re-running it only changes
    // anything once the fit crosses a pixel boundary — the normal sub-pixel move would resample
    // exactly the same ring. Rare, so this costs almost nothing; skipping it would measure flux
    // and SNR against a sky annulus centred on the wrong pixel.
    let annulus_background = if pos.round() == moments_pos.round() {
        annulus_background
    } else {
        annulus_at(pos)
    };

    // Compute quality metrics (flux, SNR, sharpness, roundness always from moments)
    let mut star = compute_star(
        pixels,
        background,
        pos,
        region.peak_value,
        stamp_radius,
        annulus_background,
        config.noise_model.as_ref(),
    )?;

    // Override FWHM and eccentricity with fit-derived values when available
    if let Some(fwhm) = fit_fwhm {
        star.fwhm = fwhm;
    }
    if let Some(ecc) = fit_eccentricity {
        star.eccentricity = ecc;
    }

    Some(star)
}

/// Single iteration of centroid refinement using Gaussian-weighted moments.
///
/// Returns the new position or None if position is invalid.
/// Uses f64 accumulators for numerical stability.
fn refine_centroid(
    pixels: &Buffer2<f32>,
    background: &BackgroundEstimate,
    pos: DVec2,
    stamp_radius: usize,
    expected_fwhm: f32,
) -> Option<DVec2> {
    let size = Size2us::new(pixels.width(), pixels.height());
    if !is_valid_stamp_position(pos, size, stamp_radius) {
        return None;
    }

    let icx = pos.x.round() as isize;
    let icy = pos.y.round() as isize;

    // Adaptive sigma based on expected FWHM
    // sigma ≈ FWHM / FWHM_TO_SIGMA, use 0.8× for tighter weighting to reduce noise
    let sigma = (expected_fwhm / FWHM_TO_SIGMA * 0.8).clamp(1.0, stamp_radius as f32 * 0.5);
    let two_sigma_sq = 2.0 * (sigma as f64) * (sigma as f64);

    let mut sum_x = 0.0f64;
    let mut sum_y = 0.0f64;
    let mut sum_w = 0.0f64;

    let pos_x = pos.x;
    let pos_y = pos.y;

    let stamp_radius_i32 = stamp_radius as i32;
    let stamp_size = 2 * stamp_radius + 1;
    let x0 = icx as usize - stamp_radius;

    // The weight is a circular Gaussian, so it factors per axis:
    // `exp(-(dx² + dy²)/2σ²) = exp(-dx²/2σ²) · exp(-dy²/2σ²)`. Filling one column vector here and
    // one row scalar below turns `(2r+1)²` `exp` calls into `2(2r+1)` — 961 into 62 at the largest
    // stamp — and this loop is what `measure_star` spends most of its time in. Costs one extra
    // multiply per pixel and a ulp or two of weight precision against the unfactored form.
    //
    // `column_moments[c]` is `c · column_weights[c]`, prebuilt so the inner loop is two dot
    // products over the row and never converts a pixel index to f64.
    let mut column_weights = [0.0f64; MAX_STAMP_SIZE];
    let mut column_moments = [0.0f64; MAX_STAMP_SIZE];
    for column in 0..stamp_size {
        let ddx = (x0 + column) as f64 - pos_x;
        let weight = (-ddx * ddx / two_sigma_sq).exp();
        column_weights[column] = weight;
        column_moments[column] = column as f64 * weight;
    }

    for dy in -stamp_radius_i32..=stamp_radius_i32 {
        let y = (icy + dy as isize) as usize;
        // One bounds check per row rather than per pixel — `is_valid_stamp_position` above has
        // already established the whole stamp is inside the frame.
        let px_row = &pixels.row(y)[x0..x0 + stamp_size];
        let bg_row = &background.background.row(y)[x0..x0 + stamp_size];

        let py = y as f64;
        let ddy = py - pos_y;
        let row_weight = (-ddy * ddy / two_sigma_sq).exp();

        // The row's total weight, and its first moment about the stamp's left edge. `row_weight`
        // and `py` are constant across the row, so they scale these two totals once at the bottom
        // instead of multiplying into every pixel.
        let mut row_w = 0.0f64;
        let mut row_x = 0.0f64;
        for (((&value, &sky), &column_weight), &column_moment) in px_row
            .iter()
            .zip(bg_row)
            .zip(&column_weights[..stamp_size])
            .zip(&column_moments[..stamp_size])
        {
            let signal = f64::from((value - sky).max(0.0));
            row_w += signal * column_weight;
            row_x += signal * column_moment;
        }

        let weighted_row = row_weight * row_w;
        sum_w += weighted_row;
        sum_y += weighted_row * py;
        sum_x += row_weight * row_x;
    }

    if sum_w < f64::EPSILON {
        return None;
    }

    // `sum_x` is the first moment about the stamp's left edge, so lift it back into image x.
    let new_pos = DVec2::new(x0 as f64 + sum_x / sum_w, sum_y / sum_w);

    // Reject if centroid moved too far (likely bad detection)
    let max_move = stamp_size as f64 / 4.0;
    if (new_pos - pos).abs().max_element() > max_move {
        return None;
    }

    Some(new_pos)
}

/// Symmetric 2×2 covariance (px²) for windowed second moments.
#[derive(Debug, Clone, Copy)]
struct Cov2 {
    xx: f64,
    yy: f64,
    xy: f64,
}

impl Cov2 {
    fn trace(self) -> f64 {
        self.xx + self.yy
    }

    fn det(self) -> f64 {
        self.xx * self.yy - self.xy * self.xy
    }

    /// Inverse of the symmetric matrix, or `None` if (near-)singular.
    fn inverse(self) -> Option<Cov2> {
        let det = self.det();
        if det.abs() < 1e-12 {
            return None;
        }
        let inv = 1.0 / det;
        Some(Cov2 {
            xx: self.yy * inv,
            yy: self.xx * inv,
            xy: -self.xy * inv,
        })
    }
}

/// Window-scale bounds (px²): σ ∈ [0.5, 10] px.
const MIN_SIGMA_SQ: f64 = 0.25;
const MAX_SIGMA_SQ: f64 = 100.0;

/// Adaptive windowed second moments (SExtractor WIN style).
///
/// Weights the second moments by a circular Gaussian whose scale is iterated to
/// match the source (`σ_w² → trace(C)/2`), exponentially suppressing far-wing
/// noise, then deconvolves the window — `C = (C_obs⁻¹ − σ_w⁻²·I)⁻¹` — so the
/// result stays unbiased. Uses the unclamped signed `(px − bg)`: the window already
/// kills the wings, so noise cancels instead of rectifying and inflating
/// eccentricity (the failure mode of plain signed moments over a fixed stamp).
///
/// Returns the source covariance, or `None` if it never reaches a valid
/// positive-definite estimate (caller falls back to the plain moments).
///
/// `background_override` replaces the per-pixel map with a flat stamp-level sky,
/// exactly as in [`compute_star`] — both must subtract the same background or
/// FWHM/eccentricity and flux/SNR would come from different sky conventions.
///
/// The whole stamp must lie inside the frame; this indexes rows and columns unchecked, so a
/// position nearer the edge than `stamp_radius` underflows the column arithmetic.
fn windowed_covariance(
    pixels: &Buffer2<f32>,
    background: &BackgroundEstimate,
    background_override: Option<LocalBackground>,
    pos: DVec2,
    stamp_radius: usize,
    seed_sigma_sq: f64,
) -> Option<Cov2> {
    const MAX_ITERS: usize = 4;

    // Caller's contract, not data validation: `compute_star` has already rejected edge positions.
    debug_assert!(
        is_valid_stamp_position(
            pos,
            Size2us::new(pixels.width(), pixels.height()),
            stamp_radius
        ),
        "windowed_covariance needs the whole stamp in frame: pos {pos}, radius {stamp_radius}"
    );

    let icx = pos.x.round() as isize;
    let icy = pos.y.round() as isize;
    let pos_x = pos.x;
    let pos_y = pos.y;
    let sr = stamp_radius as i32;

    let mut sigma_w_sq = seed_sigma_sq.clamp(MIN_SIGMA_SQ, MAX_SIGMA_SQ);
    let mut best: Option<Cov2> = None;

    let stamp_size = 2 * stamp_radius + 1;
    // Column offsets survive every iteration; their exponentials do not, because `inv_two_sw`
    // is re-derived from the matched window each pass.
    let mut column_offsets = [0.0f64; MAX_STAMP_SIZE];
    for (column, offset) in column_offsets[..stamp_size].iter_mut().enumerate() {
        *offset = (icx + column as isize - stamp_radius as isize) as f64 - pos_x;
    }

    for _ in 0..MAX_ITERS {
        let inv_two_sw = 1.0 / (2.0 * sigma_w_sq);
        let mut w_sum = 0.0f64;
        let mut mxx = 0.0f64;
        let mut myy = 0.0f64;
        let mut mxy = 0.0f64;

        // The window is circular, so `exp(-(fx² + fy²)·k)` factors per axis exactly as in
        // `refine_centroid` — `2(2r+1)` exponentials per iteration instead of `(2r+1)²`.
        let mut column_weights = [0.0f64; MAX_STAMP_SIZE];
        for (weight, &fx) in column_weights[..stamp_size]
            .iter_mut()
            .zip(&column_offsets[..stamp_size])
        {
            *weight = (-fx * fx * inv_two_sw).exp();
        }

        for dy in -sr..=sr {
            let y = (icy + dy as isize) as usize;
            let px_row = pixels.row(y);
            let bg_row = background.background.row(y);

            let fy = y as f64 - pos_y;
            let row_weight = (-fy * fy * inv_two_sw).exp();

            for (column, (&fx, &column_weight)) in column_offsets[..stamp_size]
                .iter()
                .zip(&column_weights[..stamp_size])
                .enumerate()
            {
                let x = icx as usize + column - stamp_radius;
                let bg = match background_override {
                    Some(local) => local.bg,
                    None => bg_row[x],
                };
                let wv = column_weight * row_weight * (px_row[x] - bg) as f64;
                w_sum += wv;
                mxx += wv * fx * fx;
                myy += wv * fy * fy;
                mxy += wv * fx * fy;
            }
        }

        if w_sum < f64::EPSILON {
            break;
        }
        let obs = Cov2 {
            xx: mxx / w_sum,
            yy: myy / w_sum,
            xy: mxy / w_sum,
        };

        // Deconvolve the circular window: C = (C_obs⁻¹ − σ_w⁻²·I)⁻¹
        let Some(obs_inv) = obs.inverse() else { break };
        let inv_sw = 1.0 / sigma_w_sq;
        let decon_inv = Cov2 {
            xx: obs_inv.xx - inv_sw,
            yy: obs_inv.yy - inv_sw,
            xy: obs_inv.xy,
        };
        let Some(c) = decon_inv.inverse() else { break };
        if c.det() <= 0.0 || c.trace() <= 0.0 {
            break;
        }

        let new_sigma_w_sq = (c.trace() / 2.0).clamp(MIN_SIGMA_SQ, MAX_SIGMA_SQ);
        let converged = (new_sigma_w_sq - sigma_w_sq).abs() < 1e-3 * sigma_w_sq;
        best = Some(c);
        sigma_w_sq = new_sigma_w_sq;
        if converged {
            break;
        }
    }

    best
}

/// Construct a star and compute its quality metrics at the given position.
///
/// Uses f64 accumulators for numerical stability.
///
/// If `noise_model` is provided, uses the full CCD noise equation:
/// `SNR = flux / sqrt(flux/G + npix × (σ_sky² + (read_noise_electrons/G)²))`,
/// where `G` is electrons per normalized unit.
///
/// Otherwise, uses the simplified background-dominated formula:
/// `SNR = flux / (σ_sky × sqrt(npix))`
///
/// `background_override`, when set, replaces the per-pixel tiled background/noise
/// map with a single flat estimate for the whole stamp — used for
/// [`LocalBackgroundMethod::LocalAnnulus`](crate::stacking::star_detection::config::measurement_config::LocalBackgroundMethod::LocalAnnulus),
/// whose locally-estimated sky level is only valid at the stamp scale, not
/// interpolated per pixel like the tiled map. It applies to every background
/// consumer here — flux/marginals, the windowed covariance behind FWHM/eccentricity,
/// and the SNR noise — so all metrics share one sky convention.
fn compute_star(
    pixels: &Buffer2<f32>,
    background: &BackgroundEstimate,
    pos: DVec2,
    peak: f32,
    stamp_radius: usize,
    background_override: Option<LocalBackground>,
    noise_model: Option<&NoiseModel>,
) -> Option<Star> {
    let width = pixels.width();
    let height = pixels.height();

    if !is_valid_stamp_position(pos, Size2us::new(width, height), stamp_radius) {
        return None;
    }

    let icx = pos.x.round() as isize;
    let icy = pos.y.round() as isize;

    // Collect background-subtracted values and positions (f64 accumulators)
    let mut flux = 0.0f64;
    let mut core_flux = 0.0f64;
    let mut sum_x2 = 0.0f64;
    let mut sum_y2 = 0.0f64;
    let mut sum_xy = 0.0f64;
    let mut noise_sum = 0.0f64;
    let mut noise_count = 0usize;
    let mut peak_value = 0.0f64;

    // For roundness calculation: marginal sums
    let stamp_size = 2 * stamp_radius + 1;
    let mut marginal_x = [0.0f64; MAX_STAMP_SIZE];
    let mut marginal_y = [0.0f64; MAX_STAMP_SIZE];

    let stamp_radius_i32 = stamp_radius as i32;
    let outer_ring_threshold = (stamp_radius_i32 - 2) * (stamp_radius_i32 - 2);
    for dy in -stamp_radius_i32..=stamp_radius_i32 {
        let y = (icy + dy as isize) as usize;
        let px_row = pixels.row(y);
        let bg_row = background.background.row(y);
        let noise_row = background.noise.row(y);
        for dx in -stamp_radius_i32..=stamp_radius_i32 {
            let x = (icx + dx as isize) as usize;

            let bg = match background_override {
                Some(local) => local.bg,
                None => bg_row[x],
            };
            let value = (px_row[x] - bg).max(0.0) as f64;

            flux += value;
            peak_value = peak_value.max(value);

            // Core flux for sharpness (3x3 region around center)
            if dx.abs() <= 1 && dy.abs() <= 1 {
                core_flux += value;
            }

            // Marginal distributions for roundness
            let mx_idx = (dx + stamp_radius_i32) as usize;
            let my_idx = (dy + stamp_radius_i32) as usize;
            marginal_x[mx_idx] += value;
            marginal_y[my_idx] += value;

            // Weighted second moments for FWHM and eccentricity. Kept in this loop rather than
            // recomputed on the rare `windowed_covariance` failure below: a second traversal there
            // measured no better than these three multiply-adds, which are a small share of an
            // already branchy loop body.
            let fx = x as f64 - pos.x;
            let fy = y as f64 - pos.y;
            sum_x2 += value * fx * fx;
            sum_y2 += value * fy * fy;
            sum_xy += value * fx * fy;

            // Collect noise from background region (outer ring)
            let r2 = dx * dx + dy * dy;
            if background_override.is_none() && r2 > outer_ring_threshold {
                noise_sum += noise_row[x] as f64;
                noise_count += 1;
            }
        }
    }

    if flux < f64::EPSILON {
        return None;
    }

    // Adaptive windowed second moments: Gaussian-weight by an iteratively-matched
    // window to suppress wing noise, then deconvolve the window so FWHM/eccentricity
    // stay unbiased. Seed the window from the plain moment; fall back to the plain
    // moments if it can't converge to a valid (positive-definite) covariance.
    // `sum_x2 + sum_y2` is Σ value·r², the radial moment the window is seeded from.
    let seed_sigma_sq = ((sum_x2 + sum_y2) / flux / 2.0).max(MIN_SIGMA_SQ);
    let cov = windowed_covariance(
        pixels,
        background,
        background_override,
        pos,
        stamp_radius,
        seed_sigma_sq,
    )
    .unwrap_or(Cov2 {
        xx: sum_x2 / flux,
        yy: sum_y2 / flux,
        xy: sum_xy / flux,
    });

    let trace = cov.trace();
    let det = cov.det();

    // FWHM from the mean second moment (assuming Gaussian PSF)
    let sigma_sq = (trace / 2.0).max(0.0);
    let fwhm = sigma_to_fwhm(sigma_sq.sqrt() as f32);

    // Eccentricity from covariance matrix eigenvalues
    let discriminant = (trace * trace - 4.0 * det).max(0.0);
    let lambda1 = (trace + discriminant.sqrt()) / 2.0;
    let lambda2 = (trace - discriminant.sqrt()) / 2.0;

    let eccentricity = if lambda1 > f64::EPSILON {
        (1.0 - lambda2 / lambda1).sqrt().clamp(0.0, 1.0) as f32
    } else {
        0.0
    };

    // Compute SNR using appropriate noise model
    let avg_noise = match background_override {
        Some(local) => local.noise,
        None if noise_count > 0 => (noise_sum / noise_count as f64) as f32,
        None => background.noise.row(icy as usize)[icx as usize],
    };

    let npix = (2 * stamp_radius + 1).pow(2);
    let flux_f32 = flux as f32;

    let snr = compute_snr(flux_f32, avg_noise, npix, noise_model);

    // Sharpness = peak / core_flux
    let sharpness = if core_flux > f64::EPSILON {
        (peak_value / core_flux).clamp(0.0, 1.0) as f32
    } else {
        1.0
    };

    Some(Star {
        pos,
        flux: flux_f32,
        fwhm,
        eccentricity,
        snr,
        peak,
        sharpness,
        roundness: Roundness::from_marginals(&marginal_x[..stamp_size], &marginal_y[..stamp_size]),
    })
}

/// Compute SNR using the configured sensor noise model when available.
///
/// Uses the full CCD noise equation when the model is provided:
/// `SNR = flux / sqrt(flux/G + npix × (σ_sky² + (read_noise_electrons/G)²))`,
/// where `G` is electrons per normalized unit.
///
/// Otherwise, uses simplified background-dominated formula:
/// `SNR = flux / (σ_sky × sqrt(npix))`
fn compute_snr(flux: f32, sky_noise: f32, npix: usize, noise_model: Option<&NoiseModel>) -> f32 {
    let sky_var = sky_noise * sky_noise;

    let total_var = match noise_model {
        Some(noise) => noise.variance_normalized(flux as f64, sky_noise as f64, npix) as f32,
        None => npix as f32 * sky_var,
    };

    // Floor the variance rather than branching on it: dividing by `total_var` in one arm and by
    // its square root in the other put a 2896× step at the boundary. `max` also swallows a
    // non-finite variance, since it returns the other operand for NaN.
    flux / total_var.max(f32::EPSILON).sqrt()
}
