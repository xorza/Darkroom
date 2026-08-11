//! Per-frame robust statistics, measured before any interpolation touches the pixels.

use arrayvec::ArrayVec;
use rayon::prelude::*;
use serde::{Deserialize, Serialize};

use crate::math::statistics::{MedianMad, mad_f32_with_scratch, median_f32_mut};
use crate::stacking::frame_store::StackableImage;

/// Per-frame statistics: one median/MAD pair per channel.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct FrameStats {
    pub(crate) channels: ArrayVec<MedianMad, 3>,
    pub(crate) quantization_sigma: Option<f32>,
    /// The span the frame's decoder divided by — see [`ImageMetadata::physical_scale`]. Carried
    /// beside the statistics because it is what makes two frames' statistics comparable at all.
    pub(crate) physical_scale: Option<f32>,
}

impl FrameStats {
    /// Measure per-channel median and MAD on `image`, before any interpolation touches it.
    pub(crate) fn measure(image: &impl StackableImage) -> Self {
        let dimensions = image.dimensions();
        let quantization_sigma = image.quantization_sigma();
        let physical_scale = image.metadata().physical_scale();
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
            physical_scale,
        }
    }
}
