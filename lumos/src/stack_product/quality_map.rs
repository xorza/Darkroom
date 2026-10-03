//! A stacked master's ancillary quality plane: one shared, or one per channel.

use arrayvec::ArrayVec;
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
    pub(crate) fn from_pixels(pixels: LinearPixels) -> Self {
        match pixels {
            LinearPixels::L(plane) => Self::Shared(plane),
            LinearPixels::Rgb(planes) => Self::PerChannel(planes),
        }
    }

    /// One plane per image channel: shared for one channel, per channel for three.
    ///
    /// # Panics
    /// For any other count, which no image has.
    pub(crate) fn from_planes(planes: ArrayVec<Buffer2<f32>, 3>) -> Self {
        match planes.len() {
            1 => Self::Shared(planes.into_iter().next().expect("one plane")),
            3 => Self::PerChannel(planes.into_inner().expect("three planes")),
            count => panic!("an image has one or three channels, not {count}"),
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

#[cfg(test)]
pub(crate) mod internals {
    use imaginarium::Buffer2;

    use crate::stack_product::quality_map::QualityMap;

    impl QualityMap {
        /// The quality plane applicable to an image channel.
        pub(crate) fn channel(&self, channel: usize) -> &Buffer2<f32> {
            match self {
                Self::Shared(plane) => plane,
                Self::PerChannel(planes) => &planes[channel],
            }
        }
    }
}
