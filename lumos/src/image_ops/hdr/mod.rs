//! HDR multiscale dynamic-range compression. See `hdr/README.md`.
//!
//! Reveal detail in an overexposed bright region (galaxy/nebula cores, Milky-Way star clouds) by
//! compressing the **large-scale** brightness while preserving fine detail: à trous starlet
//! decomposition, attenuate the coarse residual toward its mean, leave the detail layers, recombine.
//! A **display-domain** (post-stretch) operation. Reuses [`crate::image_ops::wavelet::StarletTransform`].

use rayon::prelude::*;

use crate::image_ops::op::{OpError, ensure, require_f32_master};
use crate::image_ops::remap_intensity;
use crate::image_ops::wavelet::{StarletTransform, max_scales};
use imaginarium::{Buffer2, Image};

#[cfg(test)]
mod tests;

/// Multiscale dynamic-range compression of a *stretched* (display-domain) image in place.
///
/// Computed on the combined intensity; color channels are rescaled hue-preservingly. Grayscale gets
/// the compressed intensity directly.
#[derive(Debug, Clone, Copy)]
pub struct Hdr {
    /// Number of wavelet scales. Structures coarser than ~`2^scales` px live in the residual and get
    /// compressed; finer detail is preserved. *More* scales → only the very largest structures
    /// compress. Clamped to what the image size supports.
    pub scales: usize,
    /// Compression strength in `[0, 1]`: `0` = no-op, `1` = the large-scale brightness is flattened
    /// to its mean.
    pub amount: f32,
}

impl Default for Hdr {
    fn default() -> Self {
        Self {
            scales: 6,
            amount: 0.5,
        }
    }
}

impl Hdr {
    /// Set the wavelet scale count.
    pub fn scales(mut self, scales: usize) -> Self {
        self.scales = scales;
        self
    }

    /// Set the compression strength in `[0, 1]`.
    pub fn amount(mut self, amount: f32) -> Self {
        self.amount = amount;
        self
    }

    /// Compress the dynamic range of `image` in place.
    ///
    /// # Errors
    /// [`OpError::UnsupportedFormat`] unless `image` is `L_F32`/`RGB_F32`; [`OpError::InvalidConfig`]
    /// on out-of-range parameters.
    pub fn apply(&self, image: &mut Image) -> Result<(), OpError> {
        self.validate()?;
        require_f32_master(image)?;
        remap_intensity(image, |intensity| hdr_map(intensity, self));
        Ok(())
    }

    fn validate(&self) -> Result<(), OpError> {
        ensure(self.scales >= 1, || {
            format!("hdr scales must be ≥ 1, got {}", self.scales)
        })?;
        ensure((0.0..=1.0).contains(&self.amount), || {
            format!("hdr amount must be in [0, 1], got {}", self.amount)
        })
    }
}

/// The starlet residual-flattening on the combined intensity plane; [`Hdr::apply`] computes the
/// intensity, runs this, then remaps the image's channels to it.
fn hdr_map(intensity: &Buffer2<f32>, config: &Hdr) -> Buffer2<f32> {
    let (w, h) = (intensity.width(), intensity.height());
    let scales = config.scales.min(max_scales(w, h));
    let mut transform = StarletTransform::forward(intensity, scales);

    // Flatten the large-scale residual toward its mean by `amount` (keep = 1 − amount of the
    // deviation), leaving the detail layers untouched.
    let residual = &mut transform.residual;
    let mean = residual.pixels().iter().sum::<f32>() / residual.len() as f32;
    let keep = 1.0 - config.amount;
    residual
        .pixels_mut()
        .par_iter_mut()
        .for_each(|v| *v = mean + keep * (*v - mean));
    transform.reconstruct()
}
