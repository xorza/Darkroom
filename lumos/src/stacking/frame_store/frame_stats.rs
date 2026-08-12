//! Per-frame robust statistics, measured before any interpolation touches the pixels.

use arrayvec::ArrayVec;
use rayon::prelude::*;
use serde::{Deserialize, Serialize};

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
}

impl FrameStats {
    /// Measure per-channel median and MAD on `image`, before any interpolation touches it.
    pub(crate) fn measure(image: &impl StackableImage) -> Self {
        let dimensions = image.dimensions();
        let quantization_sigma = image.quantization_sigma();
        let domain = image.metadata().sample_domain();
        let channels = (0..dimensions.channels())
            .into_par_iter()
            .map(|channel| {
                let data = image.channel(channel);
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
        }
    }
}
