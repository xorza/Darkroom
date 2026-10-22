//! [`RefinedBg`]: a frame's sky measured again around the sources found on it.

use imaginarium::Buffer2;

use crate::bit_buffer2::BitBuffer2;
use crate::image::masked_plane::MaskedPlane;
use crate::scratch_pool::Pooled;
use crate::star_detection::bg::bg_estimate::BgEstimate;
use crate::star_detection::detection_filter::FilterSource;
use crate::star_detection::detection_plane::{DetectionPlane, PlaneSpec};
use crate::star_detection::resources::DetectionResources;
use crate::star_detection::star_detection_config::bg_config::BgRefinement;

/// A frame's sky, and the sources it was measured around with the pixels the caller masked, none
/// when the configuration refines nothing.
#[derive(Debug)]
pub(crate) struct RefinedBg {
    pub(crate) estimate: BgEstimate,
    pub(crate) sources: Option<Pooled<BitBuffer2>>,
}

impl RefinedBg {
    /// The sky of `pixels`, leaving out `spec.mask`, refined around its sources as
    /// `spec.bg`'s refinement asks: `iterations` times, threshold the detection plane
    /// `spec` makes of the residual at `mask_sigma` of its own σ, dilate the mask by a disk of
    /// `mask_dilation`, add `spec.mask`, and measure the sky again around it. photutils masks its
    /// sources on the convolved data, at 2σ, with a circular footprint. Without a refinement the
    /// first estimate is the sky, with no sources.
    ///
    /// `iterations` is at least one by its type, and it has to be: the loop is what writes the
    /// source mask this returns, and a mask leased from the pool holds whatever its last holder
    /// left.
    pub(crate) fn measure(
        pixels: &Buffer2<f32>,
        spec: PlaneSpec<'_>,
        resources: &mut DetectionResources,
    ) -> Self {
        let estimate = |mask: Option<&BitBuffer2>, resources: &mut DetectionResources| {
            BgEstimate::estimate(
                MaskedPlane { pixels, mask },
                spec.bg,
                &resources.surfaces,
                &mut resources.bg,
            )
        };
        let mut sky = estimate(spec.mask, resources);
        let BgRefinement::Iterative(refinement) = spec.bg.refinement else {
            return Self {
                estimate: sky,
                sources: None,
            };
        };
        let mut sources = resources.acquire_bit();
        let mut scratch = resources.acquire_bit();
        for _ in 0..refinement.iterations.get() {
            // The residual reads the sky alone; its σ goes back before the plane measures its own.
            drop(sky.sky);
            let detect = DetectionPlane::filtered(
                FilterSource::Residual {
                    pixels,
                    sky: &sky.bg,
                },
                spec,
                resources,
            );
            drop(sky.bg);
            detect.threshold(refinement.mask_sigma, &mut sources);
            drop(detect);
            sources.dilate(refinement.mask_dilation, &mut scratch);
            if let Some(mask) = spec.mask {
                sources.or_with(mask);
            }
            sky = estimate(Some(&sources), resources);
        }
        Self {
            estimate: sky,
            sources: Some(sources),
        }
    }
}
