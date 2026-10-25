//! A stacked master's ancillary quality plane: one shared, or one per channel.

use common::Buffer2;
use imaginarium::PlanarImage;

use crate::image::linear_image::LinearImage;

/// A quality map that is either common to every image channel or channel-specific.
#[derive(Debug)]
pub enum QualityMap {
    /// One plane applies to every image channel.
    Shared(Buffer2<f32>),
    /// Each RGB image channel has its own plane.
    PerChannel([Buffer2<f32>; 3]),
}

impl QualityMap {
    /// The quality plane of an image channel.
    pub const fn channel(&self, channel: usize) -> &Buffer2<f32> {
        match self {
            Self::Shared(plane) => plane,
            Self::PerChannel(planes) => &planes[channel],
        }
    }
}

impl From<PlanarImage> for QualityMap {
    fn from(pixels: PlanarImage) -> Self {
        if pixels.is_mono() {
            Self::Shared(pixels.into_mono())
        } else {
            Self::PerChannel(pixels.into_rgb())
        }
    }
}

impl From<QualityMap> for LinearImage {
    fn from(map: QualityMap) -> Self {
        match map {
            QualityMap::Shared(plane) => plane.into(),
            QualityMap::PerChannel(planes) => planes.into(),
        }
    }
}
