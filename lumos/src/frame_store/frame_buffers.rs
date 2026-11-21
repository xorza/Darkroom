//! [`FrameBuffers`]: a frame in RAM on its way to its tier, and [`NewFrame`], the plain one.

use common::Buffer2;

use crate::frame::frame_quality::FrameQuality;
use crate::image::frame::{Frame, Samples};

/// A frame's buffers in RAM as a tier stores them: moved in whole on the resident tier, written
/// and handed back on the spilled one, in the type they came in, for their producer to fill again.
pub(crate) trait FrameBuffers: Sized {
    type Samples: Samples;

    fn image(&self) -> &Frame<Self::Samples>;

    /// The quality the frame carries, its planes borrowed.
    fn quality(&self) -> FrameQuality<&Buffer2<f32>>;

    /// The image and the quality planes, moved out.
    fn into_new(self) -> NewFrame<Frame<Self::Samples>>;
}

/// A frame made in RAM: its image and the quality it carries.
#[derive(Debug)]
pub(crate) struct NewFrame<I> {
    pub(crate) image: I,
    pub(crate) quality: FrameQuality<Buffer2<f32>>,
}

impl<S: Samples> FrameBuffers for NewFrame<Frame<S>> {
    type Samples = S;

    fn image(&self) -> &Frame<S> {
        &self.image
    }

    fn quality(&self) -> FrameQuality<&Buffer2<f32>> {
        self.quality.as_ref()
    }

    fn into_new(self) -> Self {
        self
    }
}
