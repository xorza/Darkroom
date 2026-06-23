//! HDR multiscale dynamic-range compression. See `hdr/README.md`.
//!
//! Reveal detail in an overexposed bright region (galaxy/nebula cores, Milky-Way star clouds) by
//! compressing the **large-scale** brightness while preserving fine detail: à trous starlet
//! decomposition, attenuate the coarse residual toward its mean, leave the detail layers, recombine.
//! A **display-domain** (post-stretch) operation. Reuses [`crate::wavelet::StarletTransform`].

use rayon::prelude::*;

use crate::image_ops::{apply_intensity_remap, intensity_plane};
use crate::wavelet::{StarletTransform, max_scales};
use imaginarium::{Buffer2, Image};

#[cfg(test)]
mod tests;

/// Parameters for [`compress_dynamic_range`].
#[derive(Debug, Clone, Copy)]
pub struct HdrConfig {
    /// Number of wavelet scales. Structures coarser than ~`2^scales` px live in the residual and get
    /// compressed; finer detail is preserved. *More* scales → only the very largest structures
    /// compress. Clamped to what the image size supports.
    pub scales: usize,
    /// Compression strength in `[0, 1]`: `0` = no-op, `1` = the large-scale brightness is flattened
    /// to its mean.
    pub amount: f32,
}

impl Default for HdrConfig {
    fn default() -> Self {
        Self {
            scales: 6,
            amount: 0.5,
        }
    }
}

impl HdrConfig {
    /// Panic on out-of-range parameters (called by [`compress_dynamic_range`]).
    pub fn validate(&self) {
        assert!(
            self.scales >= 1,
            "hdr scales must be ≥ 1, got {}",
            self.scales
        );
        assert!(
            (0.0..=1.0).contains(&self.amount),
            "hdr amount must be in [0, 1], got {}",
            self.amount
        );
    }
}

/// Compress the dynamic range of a *stretched* (display-domain) image in place.
///
/// Computed on the combined intensity; color channels are rescaled hue-preservingly. Grayscale gets
/// the compressed intensity directly.
pub fn compress_dynamic_range(image: &mut Image, config: HdrConfig) {
    config.validate();
    let intensity = intensity_plane(image);
    let mapped = hdr_map(&intensity, config);
    apply_intensity_remap(image, &intensity, &mapped);
}

/// The starlet residual-flattening on the combined intensity plane;
/// [`compress_dynamic_range`] computes the intensity, runs this, then remaps the
/// image's channels to it.
fn hdr_map(intensity: &Buffer2<f32>, config: HdrConfig) -> Buffer2<f32> {
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
