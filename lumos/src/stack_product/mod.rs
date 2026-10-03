//! The product of a stack: the calibrated, registered, combined image plus the ancillary
//! per-pixel planes that let a downstream tool measure the result rather than only view it.

pub(crate) mod coverage;
pub(crate) mod quality_map;
pub(crate) mod quality_planes;

use crate::io::image::cfa::{CfaImage, CfaType};
use crate::io::image::linear::LinearImage;
use crate::run_report::RunReport;
use crate::stack_product::coverage::Coverage;
use crate::stack_product::quality_map::QualityMap;

/// A stacked science product shared by statistical combine and drizzle.
///
/// Each plane is `Some` only when it was requested (see [`QualityPlanes`](crate::QualityPlanes))
/// and the combine could produce it. `coverage` means the same thing whichever produced it — the
/// share of frames that reached a pixel — so a reader can interpret it without knowing the entry
/// point. `weight` cannot be shared that way: it is `Σwᵢ` over whatever the producer weighted by,
/// and those weights are the algorithms' own (see the field). Statistical quality is
/// channel-specific because rejection can retain different samples in each RGB channel; monochrome
/// and drizzle quality use shared planes.
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
    /// - A statistical combine sums, per channel, each surviving frame's weight times its
    ///   confidence at that pixel. `Equal` weighting gives unit frame weights, so the sum is the
    ///   survivor count scaled by confidence. `Noise` weighting gives each frame its inverse noise
    ///   variance, not normalized, so with unit confidence the sum is the inverse variance of the
    ///   mean. `Manual` weights are relative, and so is the sum.
    /// - Drizzle sums one shared plane of geometric drop weights: how much of each input pixel's
    ///   flux landed here, times the frame weight.
    ///
    /// It is the denominator the image was divided by, under the same weights as [`Self::variance`].
    pub weight: Option<QualityMap>,
    /// The variance of each pixel's value, in the image's units squared: `Σwᵢ²·vᵢ / (Σwᵢ)²` over
    /// the samples that formed it.
    ///
    /// `vᵢ` is the sample's CCD noise model: the frame's measured background noise, plus the photon
    /// noise of the signal above the sky when the frame states its gain
    /// ([`RunReport::variance_background_only`] says when one did not), carried through
    /// normalization and divided by the warp's confidence. A statistical combine takes it at the
    /// combined value; drizzle takes it at each input pixel's value. Drizzle spreads one input
    /// pixel over several output pixels, so its noise correlates between neighbours (Fruchter &
    /// Hook 2002): this plane is each pixel's own variance, not their covariance.
    ///
    /// Absent for median output, which has no exact variance.
    pub variance: Option<QualityMap>,
    /// The mosaic pattern every frame shared, for a stack of undemosaiced sensor frames; `None`
    /// for any other stack.
    pub cfa_type: Option<CfaType>,
    /// What the combine decided on its own: the samples it left out for their flags, and the
    /// flagged ones it had to keep.
    pub report: RunReport,
}

impl StackProduct {
    /// Reinterpret a combined mosaic stack as the calibration master it is.
    ///
    /// # Panics
    ///
    /// If the product has more than one channel, or is not a stack of mosaic frames. A CFA frame
    /// is a single mosaic plane, so a stack of them is too — `CfaImage` has nowhere to put a
    /// second channel and no loader produces one.
    pub(crate) fn into_cfa_master(self) -> CfaImage {
        assert_eq!(
            self.image.channels(),
            1,
            "a CFA master must be single-channel; got {} channels",
            self.image.channels()
        );
        CfaImage {
            data: self.image.pixels.into_l(),
            cfa_type: self
                .cfa_type
                .expect("a CFA master is stacked from mosaic frames"),
            metadata: self.image.metadata,
            flags: self.image.flags,
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::internals::prelude::*;
    use crate::io::image::cfa::CfaType;
    use crate::io::raw::demosaic::bayer::CfaPattern;
    use crate::run_report::RunReport;
    use crate::stack_product::StackProduct;
    use crate::stack_product::coverage::Coverage;

    fn product(channels: usize, cfa_type: Option<CfaType>) -> StackProduct {
        let dimensions = ImageDimensions::new((2, 1), channels);
        let mut image = LinearImage::from_pixels(
            dimensions,
            (0..dimensions.sample_count()).map(|i| i as f32).collect(),
        );
        image.metadata.exposure_time = Some(30.0);
        image.metadata.quantization_sigma = Some(0.25);
        StackProduct {
            image,
            coverage: None,
            weight: None,
            variance: None,
            cfa_type,
            report: RunReport::default(),
        }
    }

    /// A mono mosaic stack becomes the master it is: the plane, the pattern, the metadata and the
    /// quantization σ all carry over.
    #[test]
    fn a_mosaic_stack_becomes_its_master() {
        let pattern = CfaType::Bayer(CfaPattern::Rggb);
        let master = product(1, Some(pattern)).into_cfa_master();
        assert_eq!(master.data.pixels(), &[0.0, 1.0]);
        assert_eq!(master.cfa_type, pattern);
        assert_eq!(master.metadata.exposure_time, Some(30.0));
        assert_eq!(master.metadata.quantization_sigma, Some(0.25));
        assert!(master.flags.is_none());
    }

    #[test]
    #[should_panic(expected = "a CFA master must be single-channel; got 3 channels")]
    fn a_colour_stack_is_no_master() {
        product(3, Some(CfaType::Mono)).into_cfa_master();
    }

    #[test]
    #[should_panic(expected = "a CFA master is stacked from mosaic frames")]
    fn a_stack_of_demosaiced_frames_is_no_master() {
        product(1, None).into_cfa_master();
    }

    /// A uniform coverage becomes an image of its one value at its size.
    #[test]
    fn uniform_coverage_becomes_a_filled_plane() {
        let image = LinearImage::from(Coverage::Uniform {
            value: 0.5,
            size: Size2us::new(3, 2),
        });
        assert_eq!(image.dimensions(), ImageDimensions::new((3, 2), 1));
        assert_eq!(image.channel(0).pixels(), &[0.5; 6]);
    }
}
