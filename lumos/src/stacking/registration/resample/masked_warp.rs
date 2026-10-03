//! Warping a frame whose source declared pixels with no measurement.
//!
//! An output pixel draws on a whole kernel footprint of source pixels, so one null under it spreads
//! across every output pixel whose window reaches it — up to 8×8 for Lanczos4. Carrying the source
//! mask through unchanged would understate that, and letting the fill under a null into the
//! interpolation would put a fabricated sample into the result at nearly full weight.
//!
//! Both are answered by the same identity. Warping the image with its nulls set to zero gives `Σ
//! wᵢ·vᵢ·validᵢ`, and warping the validity plane through the same kernel gives `Σ wᵢ·validᵢ`; their
//! ratio is the interpolation over the surviving taps alone, which is what the kernel would have
//! produced had the missing pixels never been sampled. That is normalized convolution, and it costs
//! one extra sample per output pixel plus one per channel rather than a second resampler — the
//! hand-written kernels do all of it, at the row positions every other reader shares.
//!
//! The denominator is also the answer to "how much of this pixel is real", so it folds into
//! `coverage` and carries the same reduction into `confidence`, whose pairing with coverage the
//! combine relies on.

use arrayvec::ArrayVec;
use imaginarium::Buffer2;
use rayon::prelude::*;

use crate::io::image::linear::LinearImage;
use crate::io::image::null_mask::NullMask;
use crate::math::vec2us::Vec2us;
use crate::stacking::combine::pixel_coverage::PixelCoverage;
use crate::stacking::registration::config::WarpParams;
use crate::stacking::registration::resample::row;
use crate::stacking::registration::resample::source_position::SourcePosition;

/// The sources a null-aware warp samples, prepared once per masked frame: the validity plane, and
/// every channel with its nulls set to zero, so the fill contributes nothing to a sum.
///
/// Built per masked frame rather than held in [`WarpBuffers`](super::WarpBuffers): a frame with no
/// mask must not pay planes it will never touch, and a frame with one is already the exception.
#[derive(Debug)]
pub(super) struct MaskedSources {
    validity: Buffer2<f32>,
    zeroed: ArrayVec<Buffer2<f32>, 3>,
}

impl MaskedSources {
    pub(super) fn new(image: &LinearImage, nulls: &NullMask) -> Self {
        let zeroed = (0..image.channels())
            .map(|channel| {
                let source = image.channel(channel);
                let mut zeroed = Buffer2::new_default(source.width(), source.height());
                let width = source.width();
                zeroed
                    .pixels_mut()
                    .par_chunks_mut(width)
                    .zip(source.pixels().par_chunks(width))
                    .enumerate()
                    .for_each(|(y, (row, source_row))| {
                        for (x, (value, &sample)) in row.iter_mut().zip(source_row).enumerate() {
                            *value = if nulls.is_null_at(Vec2us::new(x, y)) {
                                0.0
                            } else {
                                sample
                            };
                        }
                    });
                zeroed
            })
            .collect();
        Self {
            validity: nulls.validity_plane(),
            zeroed,
        }
    }

    /// One output row of every channel, each pixel reconstructed from its surviving taps, and the
    /// row's quality reduced by the share real data backs.
    ///
    /// Both sums are sampled with a zero border, since a division reconciles them: a border in the
    /// numerator would be data the source never held, and one in the denominator would claim
    /// support outside the frame. A pixel that does not clear the combine's own contribution floor
    /// takes the caller's border instead — the ratio there is two vanishing quantities — and its
    /// coverage, reduced with it, tells the combine not to take a sample from it.
    ///
    /// Coverage is a fraction, so its product with the support is one too. Confidence is an
    /// effective sample count rather than a fraction, and scaling it by the same figure
    /// approximates the taps it lost — but the pairing it owes coverage is exact, which is the part
    /// the combine leans on: the two reach zero together because they are scaled together.
    ///
    /// **This understates the loss for a kernel with negative lobes.** `Σ wᵢ·validᵢ` is signed,
    /// while the geometric coverage is a ratio of tap *magnitudes* — so a window that loses only a
    /// negative tap sums past one and clamps back to "fully supported" when some of it was missing.
    /// Exact at both ends whatever the kernel and exact throughout for kernels that never go
    /// negative — Nearest and Bilinear; Bicubic and the Lanczos family carry the overstatement,
    /// bounded by their negative lobes. The reconstructed sample is unaffected: dividing by the
    /// signed sum is the right normalization whatever the signs. Closing the gap needs the
    /// validity plane resampled under `|wᵢ|`, which is a second set of kernels.
    pub(super) fn warp_row(
        &self,
        positions: &[Option<SourcePosition>],
        config: WarpParams,
        support: &mut Vec<f32>,
        channel_rows: &mut [&mut [f32]],
        coverage_row: &mut [f32],
        confidence_row: &mut [f32],
    ) {
        support.clear();
        support.resize(positions.len(), 0.0);
        row::sample_row(&self.validity, positions, config.method, 0.0, support);
        for (source, output_row) in self.zeroed.iter().zip(channel_rows.iter_mut()) {
            row::sample_row(source, positions, config.method, 0.0, output_row);
            for (value, &support) in output_row.iter_mut().zip(support.iter()) {
                *value = if PixelCoverage::new(support).contributes() {
                    *value / support
                } else {
                    config.border_value
                };
            }
        }
        for ((coverage, confidence), &support) in coverage_row
            .iter_mut()
            .zip(confidence_row.iter_mut())
            .zip(support.iter())
        {
            let support = support.clamp(0.0, 1.0);
            *coverage *= support;
            *confidence *= support;
        }
    }
}
