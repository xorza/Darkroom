//! The second moments a star's shape is read from.
//!
//! FWHM and eccentricity both come from the intensity-weighted covariance of a stamp. Measuring it
//! with a fixed window biases wide stars and clips narrow ones, so the window iterates toward the
//! star's own width, and the 2×2 matrix that results is small enough to invert in closed form.

use glam::DVec2;
use imaginarium::Buffer2;

use crate::math::size2us::Size2us;
use crate::stacking::star_detection::background::background_estimate::BackgroundEstimate;
use crate::stacking::star_detection::centroid::local_background::LocalBackground;
use crate::stacking::star_detection::centroid::{MAX_STAMP_SIZE, is_valid_stamp_position};

#[derive(Debug, Clone, Copy)]
pub(super) struct Cov2 {
    pub(super) xx: f64,
    pub(super) yy: f64,
    pub(super) xy: f64,
}

impl Cov2 {
    pub(super) fn trace(self) -> f64 {
        self.xx + self.yy
    }

    pub(super) fn det(self) -> f64 {
        self.xx * self.yy - self.xy * self.xy
    }

    /// Inverse of the symmetric matrix, or `None` if (near-)singular.
    pub(super) fn inverse(self) -> Option<Cov2> {
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
pub(super) const MIN_SIGMA_SQ: f64 = 0.25;
pub(super) const MAX_SIGMA_SQ: f64 = 100.0;

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
/// exactly as in [`compute_star`](super::compute_star) — both must subtract the same background or
/// FWHM/eccentricity and flux/SNR would come from different sky conventions.
///
/// The whole stamp must lie inside the frame; this indexes rows and columns unchecked, so a
/// position nearer the edge than `stamp_radius` underflows the column arithmetic.
pub(super) fn windowed_covariance(
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
