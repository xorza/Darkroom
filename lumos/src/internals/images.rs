//! Image and plane builders for tests.
//!
//! Only the two that assemble something: a `LinearImage` needs its `ImageDimensions` built and
//! its channels laid out planar, which is worth a name. Plain planes are left to
//! `Buffer2::new_filled` / `new` / `new_default`, which are already named constructors doing
//! three different things — wrapping them here would add a name without removing a choice.

use common::Buffer2;

use crate::image::channel_layout::ChannelLayout;
use crate::image::image_dimensions::ImageDimensions;
use crate::image::linear_image::LinearImage;
use common::Size2us;

/// Single-channel image from explicit pixel values, row-major.
pub(crate) fn mono_image(size: Size2us, pixels: Vec<f32>) -> LinearImage {
    LinearImage::from(Buffer2::new(size, pixels))
}

/// Three-channel image from planar values, one `Vec` per channel: red, green and blue.
pub(crate) fn rgb_image(size: Size2us, channels: [Vec<f32>; 3]) -> LinearImage {
    LinearImage::from_planar_channels(
        ImageDimensions::new((size.width, size.height), ChannelLayout::Rgb),
        channels,
    )
}
