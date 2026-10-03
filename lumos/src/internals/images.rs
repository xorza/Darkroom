//! Image and plane builders for tests.
//!
//! Only the two that assemble something: a `LinearImage` needs its `ImageDimensions` built and
//! its channels laid out planar, which is worth a name. Plain planes are left to
//! `Buffer2::new_filled` / `new` / `new_default`, which are already named constructors doing
//! three different things — wrapping them here would add a name without removing a choice.

use crate::io::image::image_dimensions::ImageDimensions;
use crate::io::image::linear::LinearImage;
use crate::math::size2us::Size2us;
use imaginarium::Buffer2;

/// Single-channel image from explicit pixel values, row-major.
pub(crate) fn gray_image(size: Size2us, pixels: Vec<f32>) -> LinearImage {
    LinearImage::from(Buffer2::new(size.width, size.height, pixels))
}

/// Three-channel image from planar values, one `Vec` per channel.
pub(crate) fn rgb_image(
    size: Size2us,
    red: Vec<f32>,
    green: Vec<f32>,
    blue: Vec<f32>,
) -> LinearImage {
    LinearImage::from_planar_channels(
        ImageDimensions::new((size.width, size.height), 3),
        [red, green, blue],
    )
}
