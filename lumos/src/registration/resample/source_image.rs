//! [`SourceImage`]: the frame a warp reads, wherever its planes are.

use arrayvec::ArrayVec;

use crate::io::image::image_dimensions::ImageDimensions;
use crate::io::image::linear::LinearImage;
use crate::io::image::pixel_flags::PixelFlags;
use crate::math::size2us::Size2us;

/// A frame as the warp reads it: its planes and its flags, borrowed from a `LinearImage` in memory.
#[derive(Debug)]
pub(crate) struct SourceImage<'a> {
    pub(crate) dimensions: ImageDimensions,
    pub(crate) planes: ArrayVec<SourcePlane<'a>, 3>,
    pub(crate) flags: Option<&'a PixelFlags>,
}

/// One plane of a [`SourceImage`]: its samples row by row, and the row length.
#[derive(Debug, Clone, Copy)]
pub(crate) struct SourcePlane<'a> {
    pub(crate) pixels: &'a [f32],
    pub(crate) width: usize,
}

impl<'a> SourceImage<'a> {
    pub(crate) fn of(image: &'a LinearImage) -> Self {
        let width = image.width();
        Self {
            dimensions: image.dimensions(),
            planes: (0..image.channels())
                .map(|channel| SourcePlane {
                    pixels: image.channel(channel).pixels(),
                    width,
                })
                .collect(),
            flags: image.flags.as_ref(),
        }
    }

    pub(crate) const fn size(&self) -> Size2us {
        self.dimensions.size()
    }
}
