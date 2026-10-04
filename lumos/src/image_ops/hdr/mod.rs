//! HDR multiscale dynamic-range compression.
//!
//! Reveal detail in an overexposed bright region (galaxy/nebula cores, Milky-Way star clouds) by
//! compressing the **large-scale** brightness while preserving fine detail. After Durand & Dorsey
//! (2002), the compression acts on the base layer in the log domain: the à trous starlet residual
//! of the log intensity is attenuated toward its mean, and the detail layers are left. In the log
//! domain the result is a smooth factor on each pixel, so a pixel stays positive and keeps its
//! ratio to its neighbours: a linear base compressed by subtraction instead takes a faint halo
//! below black and turns near-black noise into colour speckle. A **display-domain** (post-stretch)
//! operation, streaming [`crate::math::wavelet::atrous_smooth`] — see [`hdr_map`] for why the
//! layer pyramid is never materialized.

use common::Introspect;
use rayon::prelude::*;

use crate::error::InvalidConfigField;
use crate::image_ops::SAMPLES_PER_BLOCK;
use crate::image_ops::error::OpError;
use crate::io::image::linear::LinearImage;
use crate::math::size2us::Size2us;
use crate::math::sum;
use crate::math::wavelet::{atrous_smooth, max_scales};
use imaginarium::Buffer2;
use std::mem;

/// The least intensity the log base reads: one 16-bit display step. A pixel below it is black on
/// any display, and the floor bounds the factor a pixel takes by `65536^amount`.
const LOG_FLOOR: f32 = 1.0 / 65536.0;

/// Multiscale dynamic-range compression of a *stretched* (display-domain) image in place.
///
/// Computed on the combined intensity; color channels are rescaled hue-preservingly. Grayscale gets
/// the compressed intensity directly.
#[derive(Debug, Clone, Copy, Introspect)]
#[config(type_id = "36babf1d-0fda-4d5d-b4c6-ed4c13ebff6b")]
pub struct Hdr {
    /// Number of wavelet scales. Structures coarser than ~`2^scales` px live in the residual and
    /// get compressed; finer detail is preserved. *More* scales → only the very largest structures
    /// compress. Clamped to what the image size supports.
    pub scales: usize,
    /// Compression strength in `[0, 1]`: `0` = no-op, `1` = the large-scale brightness is flattened
    /// to its geometric mean.
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

/// The starlet residual-flattening on the log of the combined intensity plane; [`Hdr::apply`]
/// computes the intensity, runs this, then remaps the image's channels to it.
///
/// With `L = ln max(I, LOG_FLOOR)`, the detail layers are untouched by this op, so with the exact
/// starlet identity `L == residual + Σ layers` the reconstruction collapses algebraically:
/// `L′ = L − amount·(residual − mean)`, so `I′ = I·exp(−amount·(residual − mean))`: the pixel times
/// `(G/B)^amount`, with `B = exp(residual)` its local geometric mean and `G` the image's. The
/// starlet's kernel is positive, so the residual lies within the range of `L`, and the factor
/// within `65536^amount` either way; an intensity at or below zero stays there. Only the smoothed
/// residual is ever computed — a streaming à trous over three reused planes — never the layer
/// pyramid (`scales` full planes at ~100 MB each on a real master).
fn hdr_map(intensity: &Buffer2<f32>, config: &Hdr) -> Buffer2<f32> {
    let size = Size2us::new(intensity.width(), intensity.height());
    let scales = config.scales.min(max_scales(size));

    let mut c_curr = Buffer2::new(
        size.width,
        size.height,
        intensity
            .pixels()
            .par_iter()
            .map(|&i| i.max(LOG_FLOOR).ln())
            .collect(),
    );
    let mut c_next = Buffer2::new_default(size.width, size.height);
    let mut tmp = Buffer2::new_default(size.width, size.height);
    for j in 0..scales {
        atrous_smooth(&c_curr, &mut c_next, &mut tmp, 1 << j);
        mem::swap(&mut c_curr, &mut c_next);
    }
    let mut residual = c_curr;

    // Accumulated in f64: a sequential f32 fold over a 24 MP plane drifts by percents.
    let mean = sum::mean_f32(residual.pixels());
    let amount = config.amount;
    residual
        .pixels_mut()
        .par_chunks_mut(SAMPLES_PER_BLOCK)
        .zip(intensity.pixels().par_chunks(SAMPLES_PER_BLOCK))
        .for_each(|(residual, intensity)| {
            for (r, &i) in residual.iter_mut().zip(intensity) {
                *r = i * (-amount * (*r - mean)).exp();
            }
        });
    residual
}

#[cfg(test)]
mod tests;
