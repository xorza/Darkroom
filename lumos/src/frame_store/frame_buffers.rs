//! [`FrameBuffers`]: a frame in RAM on its way to its tier, and [`NewFrame`], the plain one.

use imaginarium::Buffer2;

use crate::frame::frame_quality::FrameQuality;
use crate::image::stackable_image::StackableImage;

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
