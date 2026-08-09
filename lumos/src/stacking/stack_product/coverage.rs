//! What share of the frames reached each output pixel.

use imaginarium::Buffer2;

use crate::io::image::linear::LinearImage;
use crate::math::size2us::Size2us;

/// Normalized coverage in `[0, 1]`: the share of frames that reached each pixel.
///
/// A sum type because the usual answer is a single number. When no frame carries a coverage map
/// every pixel saw every frame, and spelling that out as a plane of `1.0` costs a full image-sized
/// allocation — 240 MB at 60 MP — to carry one constant. Drizzle and warped stacks, where frames
/// genuinely cover different regions, produce [`Coverage::PerPixel`].
#[derive(Debug, Clone)]
pub enum Coverage {
    /// Every pixel covered to the same degree.
    Uniform {
        value: f32,
        /// Kept so the plane can still be materialized on demand.
        size: Size2us,
    },
    /// Coverage measured per pixel.
    PerPixel(Buffer2<f32>),
}

impl Coverage {
    /// Pixel extent of the coverage this describes.
    pub fn size(&self) -> Size2us {
        match self {
            Coverage::Uniform { size, .. } => *size,
            Coverage::PerPixel(plane) => Size2us::new(plane.width(), plane.height()),
        }
    }

    /// The measured plane, or `None` when coverage is uniform and no plane exists.
    pub fn per_pixel(&self) -> Option<&Buffer2<f32>> {
        match self {
            Coverage::Uniform { .. } => None,
            Coverage::PerPixel(plane) => Some(plane),
        }
    }

    /// Materialize as a plane. Allocates for [`Coverage::Uniform`] — the cost this type exists to
    /// let a caller avoid, so only reach for it when a plane is genuinely what is wanted.
    pub fn to_plane(&self) -> Buffer2<f32> {
        match self {
            Coverage::Uniform { value, size } => {
                Buffer2::new_filled(size.width, size.height, *value)
            }
            Coverage::PerPixel(plane) => plane.clone(),
        }
    }
}

/// Indexes like the plane it stands for, by flat sample or by `(x, y)` — a uniform coverage
/// answers with its constant rather than materializing anything.
impl std::ops::Index<usize> for Coverage {
    type Output = f32;

    fn index(&self, index: usize) -> &f32 {
        match self {
            Coverage::Uniform { value, size } => {
                debug_assert!(index < size.pixel_count(), "coverage index out of range");
                value
            }
            Coverage::PerPixel(plane) => &plane[index],
        }
    }
}

impl std::ops::Index<(usize, usize)> for Coverage {
    type Output = f32;

    fn index(&self, (x, y): (usize, usize)) -> &f32 {
        match self {
            Coverage::Uniform { value, size } => {
                debug_assert!(
                    x < size.width && y < size.height,
                    "coverage index out of range"
                );
                value
            }
            Coverage::PerPixel(plane) => &plane[(x, y)],
        }
    }
}

impl From<Coverage> for LinearImage {
    fn from(coverage: Coverage) -> Self {
        match coverage {
            Coverage::PerPixel(plane) => plane.into(),
            uniform => uniform.to_plane().into(),
        }
    }
}
