//! [`FrameBuffers`]: a frame in RAM on its way to its tier, and [`NewFrame`], the plain one.

use common::Buffer2;

use crate::frame::frame_quality::FrameQuality;
use crate::image::linear_image::LinearImage;
use crate::image::stackable_image::StackableImage;
use crate::registration::resample::WarpResult;

/// A frame's buffers in RAM as a tier stores them: moved in whole on the resident tier, written
/// and handed back on the spilled one, in the type they came in, for their producer to fill again.
pub(crate) trait FrameBuffers: Sized {
    type Image: StackableImage;

    fn image(&self) -> &Self::Image;

    /// The quality the frame carries, its planes borrowed.
    fn quality(&self) -> FrameQuality<&Buffer2<f32>>;

    /// The image and the quality planes, moved out.
    fn into_new(self) -> NewFrame<Self::Image>;
}

/// A frame made in RAM: its image and the quality it carries.
#[derive(Debug)]
pub(crate) struct NewFrame<I> {
    pub(crate) image: I,
    pub(crate) quality: FrameQuality<Buffer2<f32>>,
}

impl<I: StackableImage> FrameBuffers for NewFrame<I> {
    type Image = I;

    fn image(&self) -> &I {
        &self.image
    }

    fn quality(&self) -> FrameQuality<&Buffer2<f32>> {
        self.quality.as_ref()
    }

    fn into_new(self) -> Self {
        self
    }
}

/// A warped frame on its way to its tier: the resident tier keeps the set as the frame, and the
/// spilled one hands it back for the next frame's warp.
impl FrameBuffers for WarpResult {
    type Image = LinearImage;

    fn image(&self) -> &LinearImage {
        &self.image
    }

    fn quality(&self) -> FrameQuality<&Buffer2<f32>> {
        FrameQuality::Planes {
            coverage: &self.coverage,
            confidence: &self.confidence,
        }
    }

    fn into_new(self) -> NewFrame<LinearImage> {
        let Self {
            image,
            coverage,
            confidence,
        } = self;
        NewFrame {
            image,
            quality: FrameQuality::Planes {
                coverage,
                confidence,
            },
        }
    }
}
