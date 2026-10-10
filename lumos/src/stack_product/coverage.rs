//! What share of the frames reached each output pixel.

use imaginarium::Buffer2;

use crate::io::image::linear::LinearImage;
use crate::math::size2us::Size2us;
use crate::stack_product::quality_map::QualityMap;

/// The share of frames that reached each pixel, in `[0, 1]`.
///
/// One meaning for every producer: the frames the combine gathered at the pixel, whether a warp's
/// coverage cleared its floor there or a drizzled frame's drops landed there.
///
/// A sum type because the usual answer is a single number. When no frame carries a coverage map
/// every pixel saw every frame, and spelling that out as a plane of `1.0` costs a full image-sized
/// allocation — 240 MB at 60 MP — to carry one constant. Drizzle and warped stacks, where frames
/// genuinely cover different regions, produce [`Coverage::PerPixel`].
#[derive(Debug)]
pub enum Coverage {
    /// Every pixel covered to the same degree.
    Uniform {
        value: f32,
        /// Kept so the plane can still be materialized on demand.
        size: Size2us,
    },
    /// Coverage measured per pixel: one plane every channel shares, or one per channel for a
    /// mosaic drizzled into the channels of its colours, which reach each channel at other pixels.
    PerPixel(QualityMap),
}

impl Coverage {
    /// The coverage of image channel `channel`, materialized as a plane. Allocates for
    /// [`Coverage::Uniform`] — the cost this type exists to let a caller avoid, so only reach for
    /// it when a plane is genuinely what is wanted.
    pub fn to_plane(&self, channel: usize) -> Buffer2<f32> {
        match self {
            Coverage::Uniform { value, size } => {
                Buffer2::new_filled(size.width, size.height, *value)
            }
            Coverage::PerPixel(map) => map.channel(channel).clone(),
        }
    }
}

impl From<Coverage> for LinearImage {
    fn from(coverage: Coverage) -> Self {
        match coverage {
            Coverage::PerPixel(map) => map.into(),
            uniform @ Coverage::Uniform { .. } => uniform.to_plane(0).into(),
        }
    }
}

#[cfg(test)]
pub(crate) mod internals {
    use std::ops::Index;

    use imaginarium::Buffer2;

    use crate::stack_product::coverage::Coverage;
    use crate::stack_product::quality_map::QualityMap;

    impl Coverage {
        /// The measured planes, or `None` when coverage is uniform and no plane exists.
        pub(crate) const fn per_pixel(&self) -> Option<&QualityMap> {
            match self {
                Coverage::Uniform { .. } => None,
                Coverage::PerPixel(map) => Some(map),
            }
        }

        /// The one plane every channel shares, or the constant of a uniform coverage.
        ///
        /// # Panics
        /// For a coverage per channel, which has no one plane to read.
        fn shared(&self, read: impl FnOnce(&Buffer2<f32>) -> &f32) -> &f32 {
            match self {
                Coverage::Uniform { value, .. } => value,
                Coverage::PerPixel(QualityMap::Shared(plane)) => read(plane),
                Coverage::PerPixel(QualityMap::PerChannel(_)) => {
                    panic!("a coverage per channel has no one plane to index")
                }
            }
        }
    }

    /// Indexes like the one plane it stands for, by flat sample or by `(x, y)` — a uniform
    /// coverage answers with its constant rather than materializing anything.
    impl Index<usize> for Coverage {
        type Output = f32;

        fn index(&self, index: usize) -> &f32 {
            self.shared(|plane| &plane[index])
        }
    }

    impl Index<(usize, usize)> for Coverage {
        type Output = f32;

        fn index(&self, (x, y): (usize, usize)) -> &f32 {
            self.shared(|plane| &plane[(x, y)])
        }
    }
}
