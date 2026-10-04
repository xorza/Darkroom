//! The second moments a star's shape is read from.
//!
//! FWHM and eccentricity both come from the intensity-weighted covariance of a stamp. Measuring it
//! with a fixed window biases wide stars and clips narrow ones, so the window iterates toward the
//! star's own width, and the 2×2 matrix that results is small enough to invert in closed form.

use glam::DVec2;
use imaginarium::Buffer2;

use crate::math::fwhm::equal_area_fwhm;
use crate::math::size2us::Size2us;
use crate::star_detection::centroid::{MAX_STAMP_SIZE, stamp_centre};

#[derive(Debug, Clone, Copy)]
pub(super) struct Cov2 {
    pub(super) xx: f64,
    pub(super) yy: f64,
    pub(super) xy: f64,
}

impl Cov2 {
    pub(super) const fn trace(self) -> f64 {
        self.xx + self.yy
    }

    pub(super) const fn det(self) -> f64 {
        self.xx * self.yy - self.xy * self.xy
    }

    /// The FWHM of the circle of equal area — see [`equal_area_fwhm`].
    pub(super) fn fwhm(self) -> f32 {
        equal_area_fwhm(self.det())
    }

    /// `√(1 − λ₂/λ₁)` from the principal variances `λ₁ ≥ λ₂`: 0 for a round profile, toward 1 as
    /// it elongates, whatever its orientation.
    pub(super) fn eccentricity(self) -> f32 {
        let trace = self.trace();
        let spread = (trace * trace - 4.0 * self.det()).max(0.0).sqrt();
        let major = f64::midpoint(trace, spread);
        let minor = (trace - spread) / 2.0;
        if major > f64::EPSILON {
            (1.0 - minor / major).max(0.0).sqrt().min(1.0) as f32
        } else {
            0.0
        }
    }

    /// The covariance of the profile before the pixel integrated it, `None` when it is not
    /// positive definite: the moments of pixel samples hold the profile convolved with the unit
    /// box, which adds the box's variance 1/12 on each axis and nothing across them. Exact for
    /// unweighted moments. Through the Gaussian window, the box's kurtosis leaves `1/(480·σ²)` px²
    /// on each axis to first order: 0.0009 at σ 1.5.
    pub(super) fn less_pixel(self) -> Option<Cov2> {
        let less = Cov2 {
            xx: self.xx - PIXEL_VARIANCE,
            yy: self.yy - PIXEL_VARIANCE,
            xy: self.xy,
        };
        (less.xx > 0.0 && less.det() > 0.0).then_some(less)
    }

    /// Inverse of the symmetric matrix, or `None` if (near-)singular.
    pub(super) const fn inverse(self) -> Option<Cov2> {
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

    /// Adaptive windowed second moments (SExtractor WIN style).
    ///
    /// Weights the second moments by a circular Gaussian whose scale is iterated to
    /// match the source (`σ_w² → trace(C)/2`), exponentially suppressing far-wing
    /// noise, then deconvolves the window — `C = (C_obs⁻¹ − σ_w⁻²·I)⁻¹` — so the
    /// result stays unbiased. Uses the unclamped signed residual less `offset`: the window already
    /// kills the wings, so noise cancels instead of rectifying and inflating
    /// eccentricity (the failure mode of plain signed moments over a fixed stamp).
    ///
    /// Returns the source covariance, or `None` if it never reaches a valid
    /// positive-definite estimate (caller falls back to the plain moments).
    ///
    /// `offset` is the stamp's local sky left in the residual, exactly as in
    /// [`compute_star`](super::compute_star) — both must subtract the same one or FWHM/eccentricity and
    /// flux/SNR would come from different sky conventions.
    ///
    /// The whole stamp must lie inside the frame: a position nearer the edge than `stamp_radius` is the
    /// caller's bug, and panics.
    pub(super) fn windowed(
        residual: &Buffer2<f32>,
        offset: f32,
        pos: DVec2,
        stamp_radius: usize,
        seed_sigma_sq: f64,
    ) -> Option<Self> {
        const MAX_ITERS: usize = 4;

        // Caller's contract, not data validation: `compute_star` has already rejected edge positions.
        let centre = stamp_centre(
            pos,
            Size2us::new(residual.width(), residual.height()),
            stamp_radius,
        )
        .expect("windowed_covariance needs the whole stamp in frame");
        let (x0, y0) = (centre.x - stamp_radius, centre.y - stamp_radius);
        let pos_x = pos.x;
        let pos_y = pos.y;

        let mut sigma_w_sq = seed_sigma_sq.clamp(MIN_SIGMA_SQ, MAX_SIGMA_SQ);
        let mut best: Option<Cov2> = None;

        let stamp_size = 2 * stamp_radius + 1;
        // Column offsets survive every iteration; their exponentials do not, because `inv_two_sw`
        // is re-derived from the matched window each pass.
        let mut column_offsets = [0.0f64; MAX_STAMP_SIZE];
        for (column, offset) in column_offsets[..stamp_size].iter_mut().enumerate() {
            *offset = (x0 + column) as f64 - pos_x;
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

            for y in y0..y0 + stamp_size {
                let px_row = residual.row(y);

                let fy = y as f64 - pos_y;
                let row_weight = (-fy * fy * inv_two_sw).exp();

                for (column, (&fx, &column_weight)) in column_offsets[..stamp_size]
                    .iter()
                    .zip(&column_weights[..stamp_size])
                    .enumerate()
                {
                    let x = x0 + column;
                    let wv = column_weight * row_weight * f64::from(px_row[x] - offset);
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
}

/// The variance of a unit box, px²: what a pixel adds to the profile it integrates, on each axis.
const PIXEL_VARIANCE: f64 = 1.0 / 12.0;

/// Window-scale bounds (px²): σ ∈ [0.5, 10] px.
pub(super) const MIN_SIGMA_SQ: f64 = 0.25;
pub(super) const MAX_SIGMA_SQ: f64 = 100.0;
