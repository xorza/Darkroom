//! The product of a stack: the calibrated, registered, combined image plus the ancillary
//! per-pixel planes that let a downstream tool measure the result rather than only view it.

pub(crate) mod coverage;
pub(crate) mod quality_map;
pub(crate) mod quality_planes;

use crate::io::image::cfa::CfaImage;
use crate::io::image::linear::LinearImage;
use crate::stacking::stack_product::coverage::Coverage;
use crate::stacking::stack_product::quality_map::QualityMap;

/// A stacked science product shared by statistical combine and drizzle.
///
/// Each plane is `Some` only when it was requested (see [`QualityPlanes`](crate::QualityPlanes)) and the combine could
/// produce it. `coverage` means the same thing whichever produced it — the share of frames that
/// reached a pixel — so a reader can interpret it without knowing the entry point. `weight` cannot
/// be shared that way: it is `Σwᵢ` over whatever the producer weighted by, and those weights are
/// the algorithms' own (see the field). Statistical quality is channel-specific because rejection
/// can retain different samples in each RGB channel; monochrome and drizzle quality use shared
/// planes.
#[derive(Debug)]
pub struct StackProduct {
    /// The combined linear image.
    pub image: LinearImage,
    /// The share of frames that reached each pixel, in `[0, 1]`, for masking and fill gating.
    ///
    /// A statistical combine counts the frames whose sample cleared the coverage floor; drizzle
    /// counts the frames that deposited any flux. Neither says how much signal landed there — a
    /// pixel every frame reached through a sliver of a drop reads 1.0 — which is what `weight`
    /// answers, and what drizzle's `min_weight_fraction` gates on.
    pub coverage: Option<Coverage>,
    /// WHT map: `Σwᵢ` over whatever formed the pixel.
    ///
    /// Unlike [`Self::coverage`], this is *not* one quantity across producers, and cannot be — the
    /// two weight different things:
    ///
    /// - A statistical combine sums, per channel, each surviving frame's weight times its
    ///   confidence at that pixel. `Equal` weighting leaves unit frame weights, so the sum is the
    ///   survivor count scaled by confidence; `Noise` and `Manual` normalize the frame weights to
    ///   1 across the set first. Per channel because rejection can retain different samples in
    ///   each.
    /// - Drizzle sums one shared plane of geometric drop weights: how much of each input pixel's
    ///   flux landed here, times the frame weight.
    ///
    /// So the values are comparable *within* a product but not between producers, and a reader
    /// should treat this as relative rather than absolute. What does hold either way: it is the
    /// denominator the image was divided by, and [`Self::linear_variance`] is `Σwᵢ²/(Σwᵢ)²` over
    /// these same weights, so the two planes are always mutually consistent.
    pub weight: Option<QualityMap>,
    /// Conditional linear-combine variance factor `Σwᵢ² / (Σwᵢ)²`.
    ///
    /// Present for weighted means and drizzle, using their actual surviving/contributing samples.
    /// Absent for median output because a median is not a linear combination.
    pub linear_variance: Option<QualityMap>,
    /// Source-quantization uncertainty carried through the combine, in the stacked image's
    /// sample units.
    ///
    /// Present when every input frame declared one and the frame set carries no coverage, which
    /// is what lets a surviving sample be traced back to the frame whose sigma and normalization
    /// gain it inherited. `None` otherwise.
    pub quantization_sigma: Option<f32>,
}

impl StackProduct {
    /// Reinterpret a combined mosaic stack as the calibration master it is.
    ///
    /// # Panics
    ///
    /// If the product has more than one channel. A CFA frame is a single mosaic plane, so a
    /// stack of them is too — `CfaImage` has nowhere to put a second channel and no loader
    /// produces one.
    pub(crate) fn into_cfa_master(self) -> CfaImage {
        assert_eq!(
            self.image.channels(),
            1,
            "a CFA master must be single-channel; got {} channels",
            self.image.channels()
        );
        CfaImage {
            data: self.image.pixels.into_l(),
            metadata: self.image.metadata,
            quantization_sigma: self.quantization_sigma,
        }
    }
}
