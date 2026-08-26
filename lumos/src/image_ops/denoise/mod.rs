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

use common::{Introspect, IntrospectEnum};
use imaginarium::Buffer2;
use rayon::prelude::*;

use crate::error::InvalidConfigField;
use crate::image_ops::error::OpError;
use crate::image_ops::wavelet::{atrous_smooth, max_scales};
use crate::io::image::linear::LinearImage;
use crate::math::size2us::Size2us;
use crate::math::statistics::{mad_to_sigma, mad_with_scratch, median_mut};

#[cfg(test)]
mod tests;

/// Subsample cap for the per-scale noise estimate (uniform stride above this; exact below). A robust
/// MAD converges far below this, matching `color_calibration`'s subsampled-background precedent.
const MAX_NOISE_SAMPLES: usize = 500_000;

/// How to attenuate a wavelet coefficient that falls below the per-scale threshold.
///
/// `type_id` is this enum's identity to an introspecting consumer; that
/// consumer stores it, so it is fixed for the life of the type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, IntrospectEnum)]
#[config(type_id = "542a0fa0-25ff-4839-b309-acbe65d93a84")]
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
#[derive(Debug, Clone, Copy, Introspect)]
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
    /// [`OpError::InvalidConfig`] on out-of-range parameters.
    pub fn apply(&self, image: &mut LinearImage) -> Result<(), OpError> {
        self.validate()?;
        if self.strength == 0.0 {
            return Ok(());
        }
        let size = Size2us::new(image.width(), image.height());
        let scales = self.scales.min(max_scales(size));
        let mut scratch = DenoiseScratch::new(size);
        for plane in image.planes_mut() {
            denoise_plane(
                plane.pixels_mut(),
                scales,
                self.k,
                self.threshold,
                self.strength,
                &mut scratch,
            );
        }
        Ok(())
    }

    fn validate(&self) -> Result<(), InvalidConfigField> {
        InvalidConfigField::check(
            self.scales >= 1,
            "denoise scales",
            "at least 1",
            self.scales as f64,
        )?;
        InvalidConfigField::finite("denoise k", "finite and positive", self.k, |value| {
            value > 0.0
        })?;
        InvalidConfigField::finite(
            "denoise strength",
            "finite and in [0, 1]",
            self.strength,
            |value| (0.0..=1.0).contains(&value),
        )
    }
}

/// Reusable buffers for [`denoise_plane`], allocated once and shared across channels.
#[derive(Debug)]
struct DenoiseScratch {
    /// Current smooth `c_j` (and, after the loop, the coarse residual `c_J`).
    c_curr: Buffer2<f32>,
    /// Next smooth `c_{j+1}`.
    c_next: Buffer2<f32>,
    /// Separable-convolution horizontal-pass intermediate for [`atrous_smooth`].
    tmp: Buffer2<f32>,
    /// Subsampled coefficients for the per-scale noise estimate.
    samples: Vec<f32>,
    /// Scratch for the MAD's inner median.
    dev: Vec<f32>,
}

impl DenoiseScratch {
    fn new(size: Size2us) -> Self {
        Self {
            c_curr: Buffer2::new_default(size.width, size.height),
            c_next: Buffer2::new_default(size.width, size.height),
            tmp: Buffer2::new_default(size.width, size.height),
            samples: Vec::new(),
            dev: Vec::new(),
        }
    }
}

/// Denoise one channel in place. Reconstructs `c_J + Σ thresh(w_j)` without ever materializing all
/// planes: it starts from the original (`c_0`) and subtracts only the *removed* noise per scale, so
/// the coarse residual `c_J` is preserved implicitly (the telescoping sum `c_0 = c_J + Σ w_j`).
fn denoise_plane(
    plane: &mut [f32],
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

    c_curr.pixels_mut().copy_from_slice(plane);
    for j in 0..scales {
        let step = 1usize << j;
        atrous_smooth(c_curr, c_next, tmp, step); // c_next = c_{j+1}

        // The detail plane w_j = c_j − c_{j+1} is never materialized: its noise σ comes from a
        // strided subsample, and the threshold-removed part is recomputed inline below — saving a
        // full read+write pass over the plane each scale.
        let sigma = estimate_sigma(c_curr.pixels(), c_next.pixels(), samples, dev);
        let t = k * sigma;

        // Subtract the strength-weighted noise removed at this scale from the running result.
        plane
            .par_iter_mut()
            .zip(c_curr.pixels().par_iter())
            .zip(c_next.pixels().par_iter())
            .for_each(|((p, &cc), &cn)| {
                let w = cc - cn;
                *p -= strength * (w - threshold.apply(w, t));
            });

        std::mem::swap(c_curr, c_next); // c_curr = c_{j+1}
    }
}

/// Robust per-scale noise σ of the detail `w = curr − next`: `1.4826 · MAD` of a uniform-stride
/// subsample, computing each sampled `w` on the fly (the full detail plane is never materialized).
fn estimate_sigma(curr: &[f32], next: &[f32], samples: &mut Vec<f32>, dev: &mut Vec<f32>) -> f32 {
    let stride = (curr.len() / MAX_NOISE_SAMPLES).max(1);
    samples.clear();
    samples.extend(
        curr.iter()
            .zip(next.iter())
            .step_by(stride)
            .map(|(&c, &n)| c - n),
    );
    if samples.is_empty() {
        return 0.0;
    }
    let median = median_mut(samples);
    mad_to_sigma(mad_with_scratch(samples, median, dev))
}
