//! Warping a frame whose source declared pixels with no measurement.
//!
//! An output pixel draws on a whole kernel footprint of source pixels, so one null under it spreads
//! across every output pixel whose window reaches it — up to 8×8 for Lanczos4. Carrying the source
//! mask through unchanged would understate that, and letting the fill under a null into the
//! interpolation would put a fabricated sample into the result at nearly full weight.
//!
//! Both are answered by the same identity. Warping the image with its nulls set to zero gives
//! `Σ wᵢ·vᵢ·validᵢ`, and warping the validity plane through the same kernel gives `Σ wᵢ·validᵢ`;
//! their ratio is the interpolation over the surviving taps alone, which is what the kernel would
//! have produced had the missing pixels never been sampled. That is normalized convolution, and it
//! costs one extra plane warp per frame plus one per channel rather than a second resampler — the
//! hand-written kernels do all of it.
//!
//! The denominator is also the answer to "how much of this pixel is real", so it folds into
//! `coverage` and carries the same reduction into `confidence`, whose pairing with coverage the
//! combine relies on.

use imaginarium::Buffer2;
use rayon::prelude::*;

use crate::io::image::image_dimensions::ImageDimensions;
use crate::io::image::null_mask::NullMask;
use crate::stacking::combine::pixel_coverage::PixelCoverage;
use crate::stacking::registration::config::WarpParams;
use crate::stacking::registration::resample::plane;
use crate::stacking::registration::transform::WarpTransform;

/// The scratch a null-aware warp works through, and the support it measured.
///
/// Allocated per masked frame rather than held in [`WarpBuffers`](super::WarpBuffers): a frame with
/// no mask must not pay three planes it will never touch, and a frame with one is already the
/// exception.
///
/// Borrows the mask and the transform for the run rather than taking them per channel, so every
/// channel is guaranteed to be reconstructed against the support that was measured for it.
#[derive(Debug)]
pub(super) struct MaskedWarp<'a> {
    nulls: &'a NullMask,
    transform: &'a WarpTransform,
    /// The caller's parameters, and the same kernel filling outside the source footprint with
    /// nothing rather than their border.
    ///
    /// Both warps here are sums that a division reconciles, so the border has to be the additive
    /// identity in each: a border value in the numerator would be data the source never held, and
    /// one in the denominator would claim support outside the frame. The caller's own border is
    /// still what a pixel with no support ends up holding.
    config: WarpParams,
    zero_bordered: WarpParams,
    /// `Σ wᵢ·validᵢ` per output pixel — the share of each pixel's kernel weight that real source
    /// data backs, and the denominator every channel divides by.
    support: Buffer2<f32>,
    /// One channel at a time with its nulls zeroed, so the fill contributes nothing to the sum.
    zeroed: Buffer2<f32>,
}

impl<'a> MaskedWarp<'a> {
    /// Resample `nulls` into the support this frame's output pixels have.
    pub(super) fn measure(
        nulls: &'a NullMask,
        dimensions: ImageDimensions,
        transform: &'a WarpTransform,
        config: &WarpParams,
    ) -> Self {
        let zero_bordered = WarpParams {
            border_value: 0.0,
            ..*config
        };
        let mut support = Buffer2::new_default(dimensions.width(), dimensions.height());
        plane::warp(
            &nulls.validity_plane(),
            &mut support,
            transform,
            &zero_bordered,
        );
        Self {
            nulls,
            transform,
            config: *config,
            zero_bordered,
            support,
            zeroed: Buffer2::new_default(dimensions.width(), dimensions.height()),
        }
    }

    /// Warp one channel, reconstructing each output pixel from its surviving taps alone.
    ///
    /// A pixel that does not clear the combine's own contribution floor is border fill: the ratio
    /// there is two vanishing quantities and means nothing, and [`Self::fold_into_quality`] has
    /// already told the combine not to take a sample from it.
    pub(super) fn warp_channel(&mut self, source: &[f32], output: &mut Buffer2<f32>) {
        debug_assert_eq!(
            source.len(),
            self.zeroed.pixels().len(),
            "channel and mask geometry must agree"
        );
        let nulls = self.nulls;
        self.zeroed
            .pixels_mut()
            .par_iter_mut()
            .enumerate()
            .for_each(|(index, value)| {
                *value = if nulls.is_null(index) {
                    0.0
                } else {
                    source[index]
                };
            });
        plane::warp(&self.zeroed, output, self.transform, &self.zero_bordered);

        let border = self.config.border_value;
        output
            .pixels_mut()
            .par_iter_mut()
            .zip(self.support.pixels().par_iter())
            .for_each(|(value, &support)| {
                *value = if PixelCoverage::new(support).contributes() {
                    *value / support
                } else {
                    border
                };
            });
    }

    /// Reduce the geometric maps by the share of each pixel that real data backs.
    ///
    /// Coverage is a fraction, so the product is one too. Confidence is an effective sample count
    /// rather than a fraction, and scaling it by the same figure is an approximation of the taps it
    /// actually lost — but the pairing it owes coverage is exact, which is the part the combine
    /// leans on: the two reach zero together because they are scaled together.
    ///
    /// **This understates the loss for a kernel with negative lobes.** `Σ wᵢ·validᵢ` is signed,
    /// while the geometric coverage beside it is a ratio of tap *magnitudes* — so a window that
    /// loses only a negative tap sums past one and clamps back to "fully supported" when some of it
    /// was in fact missing. Exact at both ends whatever the kernel (every tap valid is `1`, none
    /// valid is `0`) and exact throughout for kernels that never go negative — Nearest and
    /// Bilinear. Bicubic and the Lanczos family carry the overstatement, bounded by their negative
    /// lobes, which are small against the centre tap.
    ///
    /// The reconstructed sample is unaffected: dividing by the signed sum is the right
    /// normalization whatever the signs, so [`Self::warp_channel`] stays exact for every kernel.
    /// Closing the gap needs the validity plane resampled under `|wᵢ|`, which is a second set of
    /// kernels rather than a reuse of these.
    pub(super) fn fold_into_quality(
        &self,
        coverage: &mut Buffer2<f32>,
        confidence: &mut Buffer2<f32>,
    ) {
        coverage
            .pixels_mut()
            .par_iter_mut()
            .zip(confidence.pixels_mut().par_iter_mut())
            .zip(self.support.pixels().par_iter())
            .for_each(|((coverage, confidence), &support)| {
                let support = support.clamp(0.0, 1.0);
                *coverage *= support;
                *confidence *= support;
            });
    }
}
