//! What share of the frames reached each output pixel.

use common::Buffer2;
use imaginarium::PlanarImage;

use crate::image::flags::quality_map::QualityMap;
use crate::image::linear_image::LinearImage;
use common::USize2;

/// The share of frames that reached each pixel, in `[0, 1]`.
///
/// One meaning for every producer: the frames the combine gathered at the pixel, whether a warp's
/// coverage cleared its floor there or a drizzled frame's drops landed there.
///
/// A sum type because the usual answer is a single number. When no frame carries a coverage map
/// every pixel saw every frame, and spelling that out as a plane of `1.0` costs a full image-sized
/// allocation — 240 MiB at 60 MP — to carry one constant. Drizzle and warped stacks, where frames
/// genuinely cover different regions, produce [`Coverage::PerPixel`].
#[derive(Debug)]
pub enum Coverage {
    /// Every frame reached every pixel: a coverage of 1 throughout, at the image's size.
    Full,
    /// Coverage measured per pixel: one plane every channel shares, or one per channel for a
    /// mosaic drizzled into the channels of its colors, which reach each channel at other pixels.
    PerPixel(QualityMap),
}

impl Coverage {
    /// The coverage of image channel `channel` of an image of `size`, materialized as a plane.
    /// Allocates for [`Coverage::Full`] — the cost this type exists to let a caller avoid, so only
    /// reach for it when a plane is genuinely what is wanted.
    pub fn to_plane(&self, channel: usize, size: USize2) -> Buffer2<f32> {
        match self {
            Self::Full => Buffer2::new_filled(size, 1.0),
            Self::PerPixel(map) => {
                debug_assert_eq!(map.channel(channel).size(), size);
                map.channel(channel).clone()
            }
        }
    }

    /// The coverage as an image of `size`: its planes, or one plane of 1 for [`Coverage::Full`].
    pub fn into_image(self, size: USize2) -> LinearImage {
        match self {
            Self::Full => LinearImage::from(PlanarImage::new_mono(Buffer2::new_filled(size, 1.0))),
            Self::PerPixel(map) => map.into(),
        }
    }
}

#[cfg(test)]
pub(crate) mod internals {
    use std::ops::Index;

    use common::Buffer2;

    use crate::image::flags::quality_map::QualityMap;
    use crate::stack_product::coverage::Coverage;

    impl Coverage {
        /// The measured planes, or `None` when coverage is full and no plane exists.
        pub(crate) const fn per_pixel(&self) -> Option<&QualityMap> {
            match self {
                Self::Full => None,
                Self::PerPixel(map) => Some(map),
            }
        }

        /// The one plane every channel shares, or the 1 of a full coverage.
        ///
        /// # Panics
        /// For a coverage per channel, which has no one plane to read.
        fn shared(&self, read: impl FnOnce(&Buffer2<f32>) -> &f32) -> &f32 {
            match self {
                Self::Full => &1.0,
                Self::PerPixel(QualityMap::Shared(plane)) => read(plane),
                Self::PerPixel(QualityMap::PerChannel(_)) => {
                    panic!("a coverage per channel has no one plane to index")
                }
            }
        }
    }

    /// Indexes like the one plane it stands for, by flat sample or by `(x, y)` — a uniform
    /// coverage answers with its 1 rather than materializing anything.
    impl Index<usize> for Coverage {
        type Output = f32;

        fn index(&self, idx: usize) -> &f32 {
            self.shared(|plane| &plane[idx])
        }
    }

    impl Index<(usize, usize)> for Coverage {
        type Output = f32;

        fn index(&self, (x, y): (usize, usize)) -> &f32 {
            self.shared(|plane| &plane[(x, y)])
        }
    }
}
