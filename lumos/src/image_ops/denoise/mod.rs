//! Denoising: à trous (starlet) wavelet thresholding of the linear master.
//!
//! [`Denoise`] decomposes each channel into a redundant, shift-invariant multiscale (starlet)
//! pyramid — a B3-spline à trous transform — and zeroes (hard) or shrinks (soft) the coefficients
//! below `k·σ_j`. The kept coefficients plus the untouched coarse residual reconstruct a denoised
//! channel.
//!
//! `σ_j` is the channel's white noise σ_I times scale `j`'s response to white noise
//! ([`wavelet::white_noise_sigma`]): what pure noise of σ_I leaves in that scale, the threshold
//! Starck & Murtagh's multiresolution support uses. A σ read off each scale's own coefficients
//! instead counts the nebulae and gradients there as noise at the coarse scales, and thresholds
//! faint filaments away. σ_I is the multiresolution estimate of the channel, or the stack's own
//! variance at each pixel when [`Denoise::apply_with_variance`] has it.
//!
//! A **linear-domain** operation: run after stacking and color calibration, before the stretch (the
//! stretch's non-uniform gain would distort the noise statistics this relies on).

use common::{Introspect, IntrospectEnum};
use imaginarium::Buffer2;
use rayon::prelude::*;

use crate::error::InvalidConfigField;
use crate::image_ops::SAMPLES_PER_BLOCK;
use crate::image_ops::error::OpError;
use crate::io::image::linear::LinearImage;
use crate::io::image::pixel_flags::QualityFlags;
use crate::math::noise::mrs_noise::MrsNoise;
use crate::math::size2us::Size2us;
use crate::math::wavelet;
use crate::math::wavelet::{atrous_smooth, max_scales};
use crate::stack_product::quality_map::QualityMap;
use std::mem;

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
    /// Shrink every coefficient toward zero by `t` (`sign(w)·max(|w|−t, 0)`). Smoother, less
    /// ringing.
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
#[config(type_id = "ab942729-dc49-4518-aae4-9008bd33cea1")]
pub struct Denoise {
    /// Number of wavelet scales `J`. Each scale `j` targets structure ~`2^j` px wide; more scales
    /// reach larger noise (mottle) at the cost of touching more real extended signal. Clamped to
    /// what the image size supports.
    pub scales: usize,
    /// Threshold in units of each scale's noise σ. `k = 3` keeps only coefficients with a <0.27%
    /// chance of being pure noise; higher `k` smooths more aggressively.
    pub k: f32,
    /// Hard or soft (default) coefficient thresholding.
    pub threshold: Threshold,
    /// Blend of the denoised result with the original, in `[0, 1]`: `1` = full denoise, `0` =
    /// no-op. Applied as a fraction of the removed noise, so it's a single global strength dial.
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
    /// Denoise every channel of `image` in place via starlet wavelet thresholding.
    ///
    /// # Errors
    /// [`OpError::InvalidConfig`] on out-of-range parameters.
    pub fn apply(&self, image: &mut LinearImage) -> Result<(), OpError> {
        self.apply_with_noise(image, None)
    }

    /// [`Self::apply`] with the threshold at each pixel scaled by the stack's own σ there, the
    /// root of its variance plane: low-coverage edges, noisier than the middle, are smoothed as
    /// much as their noise needs. A pixel of zero variance, which no frame reached, is left as it
    /// is.
    ///
    /// The threshold of a coefficient takes the variance at its own pixel, which is the variance of
    /// its neighbours too where the plane changes slowly. Drizzle spreads one input pixel over
    /// several output pixels, so a drizzled stack's noise correlates between neighbours and its
    /// coefficients carry less noise than this assumes.
    ///
    /// # Errors
    /// [`OpError::InvalidConfig`] on out-of-range parameters.
    ///
    /// # Panics
    /// If `variance` is not the image's size, or has another channel count.
    pub fn apply_with_variance(
        &self,
        image: &mut LinearImage,
        variance: &QualityMap,
    ) -> Result<(), OpError> {
        self.apply_with_noise(image, Some(variance))
    }

    fn apply_with_noise(
        &self,
        image: &mut LinearImage,
        variance: Option<&QualityMap>,
    ) -> Result<(), OpError> {
        self.validate()?;
        if self.strength == 0.0 {
            return Ok(());
        }
        let size = Size2us::new(image.width(), image.height());
        let scales = self.scales.min(max_scales(size));
        let mut scratch = DenoiseScratch::new(size);
        let flags = image.flags.clone();
        let excluded = |index: usize| {
            flags
                .as_ref()
                .is_some_and(|flags| flags.at(index) != QualityFlags::default())
        };
        for (channel, plane) in image.planes_mut().enumerate() {
            let noise = match variance {
                Some(variance) => {
                    let plane = variance.channel(channel);
                    assert_eq!(
                        (plane.width(), plane.height()),
                        (size.width, size.height),
                        "the variance plane is the image's size"
                    );
                    PlaneNoise::Variance(plane.pixels())
                }
                None => PlaneNoise::White(MrsNoise::estimate(plane.pixels(), size, excluded)),
            };
            self.denoise_plane(plane.pixels_mut(), scales, noise, &mut scratch);
        }
        Ok(())
    }

    /// Denoise one channel in place. Reconstructs `c_J + Σ thresh(w_j)` without ever materializing all
    /// planes: it starts from the original (`c_0`) and subtracts only the *removed* noise per scale, so
    /// the coarse residual `c_J` is preserved implicitly (the telescoping sum `c_0 = c_J + Σ w_j`).
    fn denoise_plane(
        &self,
        plane: &mut [f32],
        scales: usize,
        noise: PlaneNoise<'_>,
        scratch: &mut DenoiseScratch,
    ) {
        let Self {
            k,
            threshold,
            strength,
            ..
        } = *self;
        let DenoiseScratch {
            c_curr,
            c_next,
            tmp,
        } = scratch;

        c_curr.pixels_mut().copy_from_slice(plane);
        for j in 0..scales {
            let step = 1usize << j;
            atrous_smooth(c_curr, c_next, tmp, step); // c_next = c_{j+1}

            // The detail plane w_j = c_j − c_{j+1} is never materialized: the threshold-removed part
            // is computed inline, saving a full read+write pass over the plane each scale.
            let per_sigma = k * wavelet::white_noise_sigma(j) as f32;
            let removed = |p: &mut f32, cc: f32, cn: f32, t: f32| {
                let w = cc - cn;
                *p -= strength * (w - threshold.apply(w, t));
            };
            match noise {
                PlaneNoise::White(sigma) => {
                    let t = per_sigma * sigma;
                    plane
                        .par_chunks_mut(SAMPLES_PER_BLOCK)
                        .zip(c_curr.pixels().par_chunks(SAMPLES_PER_BLOCK))
                        .zip(c_next.pixels().par_chunks(SAMPLES_PER_BLOCK))
                        .for_each(|((plane, curr), next)| {
                            for ((p, &cc), &cn) in plane.iter_mut().zip(curr).zip(next) {
                                removed(p, cc, cn, t);
                            }
                        });
                }
                PlaneNoise::Variance(variance) => {
                    plane
                        .par_chunks_mut(SAMPLES_PER_BLOCK)
                        .zip(c_curr.pixels().par_chunks(SAMPLES_PER_BLOCK))
                        .zip(c_next.pixels().par_chunks(SAMPLES_PER_BLOCK))
                        .zip(variance.par_chunks(SAMPLES_PER_BLOCK))
                        .for_each(|(((plane, curr), next), variance)| {
                            for (((p, &cc), &cn), &v) in
                                plane.iter_mut().zip(curr).zip(next).zip(variance)
                            {
                                removed(p, cc, cn, per_sigma * v.max(0.0).sqrt());
                            }
                        });
                }
            }

            mem::swap(c_curr, c_next); // c_curr = c_{j+1}
        }
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

/// The noise one channel's thresholds scale with.
#[derive(Debug, Clone, Copy)]
enum PlaneNoise<'a> {
    /// One white-noise σ for the whole channel.
    White(f32),
    /// The variance at each pixel.
    Variance(&'a [f32]),
}

/// Reusable buffers for [`Denoise::denoise_plane`], allocated once and shared across channels.
#[derive(Debug)]
struct DenoiseScratch {
    /// Current smooth `c_j` (and, after the loop, the coarse residual `c_J`).
    c_curr: Buffer2<f32>,
    /// Next smooth `c_{j+1}`.
    c_next: Buffer2<f32>,
    /// Separable-convolution horizontal-pass intermediate for [`atrous_smooth`].
    tmp: Buffer2<f32>,
}

impl DenoiseScratch {
    fn new(size: Size2us) -> Self {
        Self {
            c_curr: Buffer2::new_default(size.width, size.height),
            c_next: Buffer2::new_default(size.width, size.height),
            tmp: Buffer2::new_default(size.width, size.height),
        }
    }
}

#[cfg(test)]
mod tests;
