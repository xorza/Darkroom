//! Denoising: à trous (starlet) wavelet thresholding of the linear master. See `denoise/README.md`
//! for the algorithm research and the rationale for this approach.
//!
//! [`Denoise`] decomposes each channel into a redundant, shift-invariant multiscale (starlet)
//! pyramid — a B3-spline à trous transform — estimates the noise per scale from the robust MAD of
//! that scale's coefficients, and zeroes (hard) or shrinks (soft) the coefficients below `k·σ`. The
//! kept coefficients plus the untouched coarse residual reconstruct a denoised channel.
//!
//! A **linear-domain** operation: run after stacking and color calibration, before the stretch (the
//! stretch's non-uniform gain would distort the noise statistics this relies on).

use imaginarium::Buffer2;
use rayon::prelude::*;

use crate::image_ops::process_planes;
use crate::math::statistics::{mad_f32_with_scratch, mad_to_sigma, median_f32_mut};
use crate::op::{OpError, ensure, require_f32_master};
use crate::wavelet::{atrous_smooth, max_scales};
use imaginarium::Image;

#[cfg(test)]
mod tests;

/// Subsample cap for the per-scale noise estimate (uniform stride above this; exact below). A robust
/// MAD converges far below this, matching `color_calibration`'s subsampled-background precedent.
const MAX_NOISE_SAMPLES: usize = 500_000;

/// How to attenuate a wavelet coefficient that falls below the per-scale threshold.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Threshold {
    /// Keep coefficients with `|w| ≥ t` unchanged, zero the rest. Preserves photometry of strong
    /// features but can ring around bright stars.
    Hard,
    /// Shrink every coefficient toward zero by `t` (`sign(w)·max(|w|−t, 0)`). Smoother, less ringing.
    Soft,
}

impl Threshold {
    #[inline]
    fn apply(self, w: f32, t: f32) -> f32 {
        match self {
            Threshold::Hard => {
                if w.abs() >= t {
                    w
                } else {
                    0.0
                }
            }
            Threshold::Soft => {
                let shrunk = w.abs() - t;
                if shrunk > 0.0 {
                    w.signum() * shrunk
                } else {
                    0.0
                }
            }
        }
    }
}

/// Wavelet denoise of a *linear* image in place: à trous starlet thresholding, per channel.
///
/// Run on linear data, after color calibration and before the stretch. No-op-safe on any size (the
/// scale count is clamped to what the dimensions support).
#[derive(Debug, Clone, Copy)]
pub struct Denoise {
    /// Number of wavelet scales `J`. Each scale `j` targets structure ~`2^j` px wide; more scales
    /// reach larger noise (mottle) at the cost of touching more real extended signal. Clamped to
    /// what the image size supports.
    pub scales: usize,
    /// Threshold in units of the per-scale noise σ. `k = 3` keeps only coefficients with a <0.27%
    /// chance of being pure noise; higher `k` smooths more aggressively.
    pub k: f32,
    /// Hard (default) or soft coefficient thresholding.
    pub threshold: Threshold,
    /// Blend of the denoised result with the original, in `[0, 1]`: `1` = full denoise, `0` = no-op.
    /// Applied as a fraction of the removed noise, so it's a single global strength dial.
    pub strength: f32,
}

impl Default for Denoise {
    fn default() -> Self {
        Self {
            scales: 2,
            k: 2.5,
            threshold: Threshold::Soft,
            strength: 0.85,
        }
    }
}

impl Denoise {
    /// Set the wavelet scale count `J`.
    pub fn scales(mut self, scales: usize) -> Self {
        self.scales = scales;
        self
    }

    /// Set the threshold in per-scale noise σ.
    pub fn k(mut self, k: f32) -> Self {
        self.k = k;
        self
    }

    /// Set hard or soft thresholding.
    pub fn threshold(mut self, threshold: Threshold) -> Self {
        self.threshold = threshold;
        self
    }

    /// Set the denoise/original blend in `[0, 1]`.
    pub fn strength(mut self, strength: f32) -> Self {
        self.strength = strength;
        self
    }

    /// Denoise every channel of `image` in place via starlet wavelet thresholding.
    ///
    /// # Errors
    /// [`OpError::UnsupportedFormat`] unless `image` is `L_F32`/`RGB_F32`; [`OpError::InvalidConfig`]
    /// on out-of-range parameters.
    pub fn apply(&self, image: &mut Image) -> Result<(), OpError> {
        self.validate()?;
        require_f32_master(image)?;
        process_planes(image, |planes| denoise_core(planes, self));
        Ok(())
    }

    fn validate(&self) -> Result<(), OpError> {
        ensure(self.scales >= 1, || {
            format!("denoise scales must be ≥ 1, got {}", self.scales)
        })?;
        ensure(self.k > 0.0 && self.k.is_finite(), || {
            format!("denoise k must be a finite value > 0, got {}", self.k)
        })?;
        ensure((0.0..=1.0).contains(&self.strength), || {
            format!("denoise strength must be in [0, 1], got {}", self.strength)
        })
    }
}

/// Denoise each channel plane (1 for L, 3 for RGB), reusing one scratch arena.
fn denoise_core(planes: &mut [Buffer2<f32>], config: &Denoise) {
    let (width, height) = (planes[0].width(), planes[0].height());
    let scales = config.scales.min(max_scales(width, height));
    let mut scratch = DenoiseScratch::new(width, height);
    for plane in planes.iter_mut() {
        denoise_plane(
            plane,
            scales,
            config.k,
            config.threshold,
            config.strength,
            &mut scratch,
        );
    }
}

/// Reusable buffers for [`denoise_plane`], allocated once and shared across channels.
#[derive(Debug)]
struct DenoiseScratch {
    /// Current smooth `c_j` (and, after the loop, the coarse residual `c_J`).
    c_curr: Buffer2<f32>,
    /// Next smooth `c_{j+1}`.
    c_next: Buffer2<f32>,
    /// Separable-convolution intermediate, reused to hold the wavelet plane `w_j = c_j − c_{j+1}`.
    tmp: Buffer2<f32>,
    /// Subsampled coefficients for the per-scale noise estimate.
    samples: Vec<f32>,
    /// Scratch for the MAD's inner median.
    dev: Vec<f32>,
}

impl DenoiseScratch {
    fn new(width: usize, height: usize) -> Self {
        Self {
            c_curr: Buffer2::new_default(width, height),
            c_next: Buffer2::new_default(width, height),
            tmp: Buffer2::new_default(width, height),
            samples: Vec::new(),
            dev: Vec::new(),
        }
    }
}

/// Denoise one channel in place. Reconstructs `c_J + Σ thresh(w_j)` without ever materializing all
/// planes: it starts from the original (`c_0`) and subtracts only the *removed* noise per scale, so
/// the coarse residual `c_J` is preserved implicitly (the telescoping sum `c_0 = c_J + Σ w_j`).
fn denoise_plane(
    plane: &mut Buffer2<f32>,
    scales: usize,
    k: f32,
    threshold: Threshold,
    strength: f32,
    scratch: &mut DenoiseScratch,
) {
    let DenoiseScratch {
        c_curr,
        c_next,
        tmp,
        samples,
        dev,
    } = scratch;

    c_curr.copy_from(plane);
    for j in 0..scales {
        let step = 1usize << j;
        atrous_smooth(c_curr, c_next, tmp, step); // c_next = c_{j+1}

        // Wavelet plane w_j = c_j − c_{j+1}, parked in tmp.
        tmp.pixels_mut()
            .par_iter_mut()
            .zip(c_curr.pixels().par_iter())
            .zip(c_next.pixels().par_iter())
            .for_each(|((w, &cc), &cn)| *w = cc - cn);

        let sigma = estimate_sigma(tmp.pixels(), samples, dev);
        let t = k * sigma;

        // Subtract the strength-weighted noise removed at this scale from the running result.
        plane
            .pixels_mut()
            .par_iter_mut()
            .zip(tmp.pixels().par_iter())
            .for_each(|(p, &w)| *p -= strength * (w - threshold.apply(w, t)));

        std::mem::swap(c_curr, c_next); // c_curr = c_{j+1}
    }
}

/// Robust per-scale noise σ: `1.4826 · MAD` of a uniform-stride subsample of the wavelet plane.
fn estimate_sigma(plane: &[f32], samples: &mut Vec<f32>, dev: &mut Vec<f32>) -> f32 {
    let stride = (plane.len() / MAX_NOISE_SAMPLES).max(1);
    samples.clear();
    samples.extend(plane.iter().step_by(stride).copied());
    if samples.is_empty() {
        return 0.0;
    }
    let median = median_f32_mut(samples);
    mad_to_sigma(mad_f32_with_scratch(samples, median, dev))
}
