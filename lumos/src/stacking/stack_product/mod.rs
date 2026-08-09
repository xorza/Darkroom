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
/// Each plane is `Some` only when it was requested (see [`QualityPlanes`]) and the combine could
/// produce it. Each producer documents how it normalizes `coverage`: statistical combine reports
/// the fraction of frames with geometric support at a pixel, while drizzle reports accumulated
/// coverage relative to its maximum. Statistical quality is channel-specific because rejection
/// can retain different samples in each RGB channel; monochrome and drizzle quality use shared
/// planes.
#[derive(Debug)]
pub struct StackProduct {
    /// The combined linear image.
    pub image: LinearImage,
    /// Normalized coverage in `[0, 1]`, for masking and fill gating.
    pub coverage: Option<Coverage>,
    /// WHT map. Statistical combines store per-channel sums of surviving frame weights multiplied
    /// by per-pixel confidence; Equal becomes survivor count at unit confidence, while
    /// Noise/Manual normalize frame weights before that multiplier. Drizzle stores one shared
    /// plane of summed geometric drop weights.
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
