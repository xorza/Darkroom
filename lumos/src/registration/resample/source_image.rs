//! [`SourceImage`]: the frame a warp reads, wherever its planes are.

use arrayvec::ArrayVec;
use imaginarium::PlanarDesc;

use crate::image::flat_gain::FlatGain;
use crate::image::image_metadata::ImageMetadata;
use crate::image::linear_image::LinearImage;
use crate::image::pixel_flags::PixelFlags;
use common::Size2us;

/// A frame as the warp reads it: its planes, its flags and its metadata, borrowed from a
/// `LinearImage` in memory.
#[derive(Debug)]
pub(crate) struct SourceImage<'a> {
    pub(crate) dimensions: PlanarDesc,
    pub(crate) metadata: &'a ImageMetadata,
    pub(crate) planes: ArrayVec<SourcePlane<'a>, 3>,
    pub(crate) flags: Option<&'a PixelFlags>,
    pub(crate) flat_gain: Option<&'a FlatGain>,
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
            dimensions: image.desc(),
            metadata: &image.metadata,
            planes: (0..image.channel_count())
                .map(|channel| SourcePlane {
                    pixels: image.channel(channel).pixels(),
                    width,
                })
                .collect(),
            flags: image.flags(),
            flat_gain: image.flat_gain().map(|gain| &**gain),
        }
    }

    pub(crate) const fn size(&self) -> Size2us {
        self.dimensions.size()
    }
}
