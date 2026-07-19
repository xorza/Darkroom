//! HDR multiscale dynamic-range compression. See `hdr/README.md`.
//!
//! Reveal detail in an overexposed bright region (galaxy/nebula cores, Milky-Way star clouds) by
//! compressing the **large-scale** brightness while preserving fine detail: à trous starlet
//! decomposition, attenuate the coarse residual toward its mean, leave the detail layers, recombine.
//! A **display-domain** (post-stretch) operation, streaming
//! [`crate::image_ops::wavelet::atrous_smooth`] — see [`hdr_map`] for why the layer pyramid is
//! never materialized.

use rayon::prelude::*;

use crate::image_ops::op::{OpError, ensure, require_f32_master};
use crate::image_ops::remap_intensity;
use crate::image_ops::wavelet::{atrous_smooth, max_scales};
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
        if self.amount == 0.0 {
            return Ok(());
        }
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
///
/// The detail layers are untouched by this op, so with the exact starlet identity
/// `intensity == residual + Σ layers` the reconstruction collapses algebraically:
/// `residual′ + Σ layers = intensity − amount·(residual − mean)`. Only the smoothed
/// residual is ever computed — a streaming à trous over three reused planes — never the
/// layer pyramid (`scales` full planes at ~100 MB each on a real master).
fn hdr_map(intensity: &Buffer2<f32>, config: &Hdr) -> Buffer2<f32> {
    let (w, h) = (intensity.width(), intensity.height());
    let scales = config.scales.min(max_scales(w, h));

    let mut c_curr = intensity.clone();
    let mut c_next = Buffer2::new_default(w, h);
    let mut tmp = Buffer2::new_default(w, h);
    for j in 0..scales {
        atrous_smooth(&c_curr, &mut c_next, &mut tmp, 1 << j);
        std::mem::swap(&mut c_curr, &mut c_next);
    }
    let mut residual = c_curr;

    let mean = residual.pixels().iter().sum::<f32>() / residual.len() as f32;
    let amount = config.amount;
    residual
        .pixels_mut()
        .par_iter_mut()
        .zip(intensity.pixels().par_iter())
        .for_each(|(r, &i)| *r = i - amount * (*r - mean));
    residual
}
