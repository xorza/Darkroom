//! The second moments a star's shape is read from.
//!
//! FWHM and eccentricity both come from the intensity-weighted covariance of a stamp. Measuring it
//! with a fixed window biases wide stars and clips narrow ones, so the window iterates toward the
//! star's own width, and the 2×2 matrix that results is small enough to invert in closed form.

use crate::math::fwhm;
use crate::star_detection::centroid::stamp::StampAt;
use crate::star_detection::centroid::windowed_stamp::{Moments, WindowedStamp};

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

    /// The FWHM of the circle of equal area — see [`fwhm::equal_area`].
    pub(super) fn fwhm(self) -> f32 {
        fwhm::equal_area(self.det())
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

    /// The covariance of the profile before the pixel integrated it: the moments of pixel samples
    /// hold the profile convolved with the unit box, which adds the box's variance 1/12 on each
    /// axis and nothing across them. Exact for unweighted moments. Through the Gaussian window, the
    /// box's kurtosis leaves `1/(480·σ²)` px² on each axis to first order: 0.0009 at σ 1.5.
    ///
    /// Each principal variance loses the 1/12 and stops at 0, so an axis narrower than a pixel
    /// reads unresolved while the other keeps its width: a point is the zero matrix, a line a pixel
    /// wide keeps its length and reads eccentricity 1. SExtractor and photutils meet the same
    /// singular case by adding 1/12 to both axes, which here would report the pixel's width as the
    /// PSF's on those sources alone.
    pub(super) fn less_pixel(self) -> Self {
        let trace = self.trace();
        let spread = (trace * trace - 4.0 * self.det()).max(0.0).sqrt();
        let major = f64::midpoint(trace, spread);
        let minor = (trace - spread) / 2.0;
        if minor >= PIXEL_VARIANCE {
            Self {
                xx: self.xx - PIXEL_VARIANCE,
                yy: self.yy - PIXEL_VARIANCE,
                xy: self.xy,
            }
        } else if major > PIXEL_VARIANCE {
            // `(C − λ₂·I)/(λ₁ − λ₂)` projects on the major axis; λ₁ > 1/12 > λ₂ keeps it defined.
            let scale = (major - PIXEL_VARIANCE) / (major - minor);
            Self {
                xx: (self.xx - minor) * scale,
                yy: (self.yy - minor) * scale,
                xy: self.xy * scale,
            }
        } else {
            Self {
                xx: 0.0,
                yy: 0.0,
                xy: 0.0,
            }
        }
    }

    /// Inverse of the symmetric matrix, or `None` if (near-)singular.
    pub(super) const fn inverse(self) -> Option<Self> {
        let det = self.det();
        if det.abs() < 1e-12 {
            return None;
        }
        let inv = 1.0 / det;
        Some(Self {
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
    /// `offset` is the stamp's local sky left in the residual.
    ///
    /// The whole stamp must lie inside the frame: a position nearer the edge than its radius is the
    /// caller's bug, and panics.
    pub(super) fn windowed(stamp: StampAt<'_>, offset: f32, seed_sigma_sq: f64) -> Option<Self> {
        const MAX_ITERS: usize = 4;

        let mut sigma_w_sq = seed_sigma_sq.clamp(MIN_SIGMA_SQ, MAX_SIGMA_SQ);
        let mut best: Option<Self> = None;
        for _ in 0..MAX_ITERS {
            let window = WindowedStamp::new(stamp, sigma_w_sq)
                .expect("Cov2::windowed needs the whole stamp in frame");
            let Some(moments) = Moments::about(&window, offset) else {
                break;
            };

            // Deconvolve the circular window: C = (C_obs⁻¹ − σ_w⁻²·I)⁻¹
            let Some(obs_inv) = moments.about_centre().inverse() else {
                break;
            };
            let inv_sw = 1.0 / sigma_w_sq;
            let decon_inv = Self {
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
const MIN_SIGMA_SQ: f64 = 0.25;
const MAX_SIGMA_SQ: f64 = 100.0;
