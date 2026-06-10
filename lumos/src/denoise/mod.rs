//! Denoising: à trous (starlet) wavelet thresholding of the linear master. See `denoise/README.md`
//! for the algorithm research and the rationale for this approach.
//!
//! [`denoise`] decomposes each channel into a redundant, shift-invariant multiscale (starlet)
//! pyramid — a B3-spline à trous transform — estimates the noise per scale from the robust MAD of
//! that scale's coefficients, and zeroes (hard) or shrinks (soft) the coefficients below `k·σ`. The
//! kept coefficients plus the untouched coarse residual reconstruct a denoised channel.
//!
//! A **linear-domain** operation: run after stacking and color calibration, before the stretch (the
//! stretch's non-uniform gain would distort the noise statistics this relies on).

use common::Buffer2;
use rayon::prelude::*;

use crate::io::astro_image::AstroImage;
use crate::math::statistics::{mad_f32_with_scratch, mad_to_sigma, median_f32_mut};

#[cfg(test)]
mod tests;

/// B3-spline low-pass filter `[1, 4, 6, 4, 1] / 16` — the separable à trous smoothing kernel.
const B3: [f32; 5] = [1.0 / 16.0, 4.0 / 16.0, 6.0 / 16.0, 4.0 / 16.0, 1.0 / 16.0];

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

/// Parameters for [`denoise`].
#[derive(Debug, Clone, Copy)]
pub struct DenoiseConfig {
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

impl Default for DenoiseConfig {
    fn default() -> Self {
        Self {
            scales: 2,
            k: 2.5,
            threshold: Threshold::Soft,
            strength: 0.85,
        }
    }
}

impl DenoiseConfig {
    /// Panic on out-of-range parameters (called by [`denoise`]).
    pub fn validate(&self) {
        assert!(
            self.scales >= 1,
            "denoise scales must be ≥ 1, got {}",
            self.scales
        );
        assert!(
            self.k > 0.0 && self.k.is_finite(),
            "denoise k must be a finite value > 0, got {}",
            self.k
        );
        assert!(
            (0.0..=1.0).contains(&self.strength),
            "denoise strength must be in [0, 1], got {}",
            self.strength
        );
    }
}

/// Denoise every channel of a *linear* image in place via starlet wavelet thresholding.
///
/// Operates per channel. No-op-safe on any size (the scale count is clamped to what the dimensions
/// support). Run on linear data, after color calibration and before the stretch.
pub fn denoise(image: &mut AstroImage, config: DenoiseConfig) {
    config.validate();
    let (width, height) = (image.width(), image.height());
    let scales = config.scales.min(max_scales(width, height));
    let mut scratch = DenoiseScratch::new(width, height);
    for c in 0..image.channels() {
        denoise_plane(
            image.channel_mut(c),
            scales,
            config.k,
            config.threshold,
            config.strength,
            &mut scratch,
        );
    }
}

/// Largest scale count `J` for which the coarsest hole step stays within the image: `2^J ≤ min(w,h)`.
/// Beyond it the à trous kernel spans the whole frame and the extra scales do nothing useful.
fn max_scales(width: usize, height: usize) -> usize {
    let min_dim = width.min(height);
    if min_dim < 2 {
        return 1;
    }
    min_dim.ilog2() as usize
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

/// One starlet smoothing step: separable B3-spline à trous convolution with hole spacing `step`,
/// `src → dst` using `tmp` as the horizontal-pass intermediate.
fn atrous_smooth(src: &Buffer2<f32>, dst: &mut Buffer2<f32>, tmp: &mut Buffer2<f32>, step: usize) {
    convolve_horizontal(src, tmp, step);
    convolve_vertical(tmp, dst, step);
}

/// Horizontal B3-spline convolution with taps at `x ± step` and `x ± 2·step` (mirror boundary).
fn convolve_horizontal(src: &Buffer2<f32>, dst: &mut Buffer2<f32>, step: usize) {
    let width = src.width();
    let wi = width as isize;
    let s = step as isize;
    dst.pixels_mut()
        .par_chunks_mut(width)
        .enumerate()
        .for_each(|(y, out)| {
            let row = src.row(y);
            for x in 0..width {
                out[x] = if x >= 2 * step && x + 2 * step < width {
                    B3[0] * row[x - 2 * step]
                        + B3[1] * row[x - step]
                        + B3[2] * row[x]
                        + B3[3] * row[x + step]
                        + B3[4] * row[x + 2 * step]
                } else {
                    let xi = x as isize;
                    B3[0] * row[reflect(xi - 2 * s, wi)]
                        + B3[1] * row[reflect(xi - s, wi)]
                        + B3[2] * row[x]
                        + B3[3] * row[reflect(xi + s, wi)]
                        + B3[4] * row[reflect(xi + 2 * s, wi)]
                };
            }
        });
}

/// Vertical B3-spline convolution with taps at `y ± step` and `y ± 2·step` (mirror boundary).
fn convolve_vertical(src: &Buffer2<f32>, dst: &mut Buffer2<f32>, step: usize) {
    let width = src.width();
    let height = src.height();
    let hi = height as isize;
    let s = step as isize;
    dst.pixels_mut()
        .par_chunks_mut(width)
        .enumerate()
        .for_each(|(y, out)| {
            let (r0, r1, r2, r3, r4) = if y >= 2 * step && y + 2 * step < height {
                (
                    src.row(y - 2 * step),
                    src.row(y - step),
                    src.row(y),
                    src.row(y + step),
                    src.row(y + 2 * step),
                )
            } else {
                let yi = y as isize;
                (
                    src.row(reflect(yi - 2 * s, hi)),
                    src.row(reflect(yi - s, hi)),
                    src.row(y),
                    src.row(reflect(yi + s, hi)),
                    src.row(reflect(yi + 2 * s, hi)),
                )
            };
            for x in 0..width {
                out[x] =
                    B3[0] * r0[x] + B3[1] * r1[x] + B3[2] * r2[x] + B3[3] * r3[x] + B3[4] * r4[x];
            }
        });
}

/// Mirror-reflect an index into `[0, n)` (whole-sample symmetric, no edge repeat), folding
/// arbitrary out-of-range values so even the coarsest hole step is safe on small images.
#[inline]
fn reflect(i: isize, n: isize) -> usize {
    debug_assert!(n >= 1);
    if n == 1 {
        return 0;
    }
    let period = 2 * (n - 1);
    let mut m = i % period;
    if m < 0 {
        m += period;
    }
    if m >= n {
        m = period - m;
    }
    m as usize
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
