//! HDR multiscale dynamic-range compression. See `hdr/README.md`.
//!
//! Reveal detail in an overexposed bright region (galaxy/nebula cores, Milky-Way star clouds) by
//! compressing the **large-scale** brightness while preserving fine detail: à trous starlet
//! decomposition, attenuate the coarse residual toward its mean, leave the detail layers, recombine.
//! A **display-domain** (post-stretch) operation, streaming
//! [`crate::image_ops::wavelet::atrous_smooth`] — see [`hdr_map`] for why the layer pyramid is
//! never materialized.

use rayon::prelude::*;

use crate::error::InvalidConfigField;
use crate::image_ops::error::OpError;
use crate::image_ops::wavelet::{atrous_smooth, max_scales};
use crate::io::image::linear::LinearImage;
use crate::math::size2us::Size2us;
use imaginarium::Buffer2;

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
    /// [`OpError::InvalidConfig`] on out-of-range parameters.
    pub fn apply(&self, image: &mut LinearImage) -> Result<(), OpError> {
        self.validate()?;
        if self.amount == 0.0 {
            return Ok(());
        }
        image.remap_intensity(|intensity| hdr_map(intensity, self));
        Ok(())
    }

    fn validate(&self) -> Result<(), InvalidConfigField> {
        InvalidConfigField::check(
            self.scales >= 1,
            "hdr scales",
            "at least 1",
            self.scales as f64,
        )?;
        InvalidConfigField::finite("hdr amount", "finite and in [0, 1]", self.amount, |value| {
            (0.0..=1.0).contains(&value)
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
    let size = Size2us::new(intensity.width(), intensity.height());
    let scales = config.scales.min(max_scales(size));

    let mut c_curr = intensity.clone();
    let mut c_next = Buffer2::new_default(size.width, size.height);
    let mut tmp = Buffer2::new_default(size.width, size.height);
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
