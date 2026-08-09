//! A stacked master's ancillary quality plane: one shared, or one per channel.

use imaginarium::Buffer2;

use crate::io::image::linear::LinearImage;
use crate::io::image::linear_pixels::LinearPixels;

/// A quality map that is either common to every image channel or channel-specific.
#[derive(Debug)]
pub enum QualityMap {
    /// One plane applies to every image channel.
    Shared(Buffer2<f32>),
    /// Each RGB image channel has its own plane.
    PerChannel([Buffer2<f32>; 3]),
}

impl QualityMap {
    /// Resolve the quality plane applicable to an image channel.
    pub fn channel(&self, channel: usize) -> &Buffer2<f32> {
        match self {
            Self::Shared(plane) => plane,
            Self::PerChannel(planes) => &planes[channel],
        }
    }

    pub(crate) fn from_pixels(pixels: LinearPixels) -> Self {
        match pixels {
            LinearPixels::L(plane) => Self::Shared(plane),
            LinearPixels::Rgb(planes) => Self::PerChannel(planes),
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
