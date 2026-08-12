//! Per-frame robust statistics, measured before any interpolation touches the pixels.

use arrayvec::ArrayVec;
use rayon::prelude::*;
use serde::{Deserialize, Serialize};

use crate::io::image::image_provenance::RowOrder;
use crate::io::image::sample_domain::SampleDomain;
use crate::math::statistics::{MedianMad, mad_f32_with_scratch, median_f32_mut};
use crate::stacking::frame_store::StackableImage;

/// Per-frame statistics: one median/MAD pair per channel.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct FrameStats {
    pub(crate) channels: ArrayVec<MedianMad, 3>,
    pub(crate) quantization_sigma: Option<f32>,
    /// What the frame's decoder said one sample is worth — see
    /// [`ImageMetadata::sample_domain`](crate::ImageMetadata::sample_domain). Carried beside the
    /// statistics because it is what makes two frames' statistics comparable at all.
    pub(crate) domain: Option<SampleDomain>,
    /// Which end of the image the frame's first stored row belongs to — see
    /// [`RowOrder`](crate::RowOrder). Carried here for the same reason as the domain: the metadata
    /// it comes from is dropped for every frame but the first, and this is what travels instead.
    pub(crate) row_order: Option<RowOrder>,
}

impl FrameStats {
    /// Measure per-channel median and MAD on `image`, before any interpolation touches it.
    ///
    /// Pixels the source declared no measurement for are left out. They matter most to the MAD: the
    /// decoder fills a null with the frame's own median, so every one of them is a zero-deviation
    /// sample, and a frame with a large masked region would report a spread far below its real
    /// noise — which is the figure weighting divides by.
    ///
    /// The other stages that measure a whole plane need no such exclusion, and the fill is why. Star
    /// detection looks for peaks above a local background and a patch sitting *at* the background
    /// produces none; the defect detectors look for outliers against a median the fill by
    /// construction is; and normalization already re-measures over the pixels every frame shares
    /// once any frame is partially covering.
    pub(crate) fn measure(image: &impl StackableImage) -> Self {
        let dimensions = image.dimensions();
        let quantization_sigma = image.quantization_sigma();
        let domain = image.metadata().sample_domain();
        let row_order = image.metadata().row_order();
        let nulls = image.nulls();
        let channels = (0..dimensions.channels())
            .into_par_iter()
            .map(|channel| {
                // Gathered only for a frame that has a mask; without one the plane itself is what
                // gets measured, and the single copy below is the scratch the median sorts in
                // place — the same one allocation this cost before nulls existed.
                let measured = nulls.map(|nulls| {
                    image
                        .channel(channel)
                        .iter()
                        .enumerate()
                        .filter(|(index, _)| !nulls.is_null(*index))
                        .map(|(_, &sample)| sample)
                        .collect::<Vec<f32>>()
                });
                let data = measured
                    .as_deref()
                    .unwrap_or_else(|| image.channel(channel));
                // A frame with nothing measured anywhere has no statistics to report. It also
                // contributes at no pixel, so what goes here is never read — but it has to be
                // something, and the median of nothing would panic.
                if data.is_empty() {
                    return MedianMad {
                        median: 0.0,
                        mad: 0.0,
                    };
                }
                let mut scratch = data.to_vec();
                let median = median_f32_mut(&mut scratch);
                let mad = mad_f32_with_scratch(data, median, &mut scratch);
                MedianMad { median, mad }
            })
            .collect::<Vec<_>>()
            .into_iter()
            .collect();
        Self {
            channels,
            quantization_sigma,
            domain,
            row_order,
        }
    }
}
