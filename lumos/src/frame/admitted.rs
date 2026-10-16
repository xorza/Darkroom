//! [`Admitted`]: a frame as its admission found it.

use crate::frame::frame_facts::FrameFacts;
use crate::frame::frame_stats::FrameStats;
use crate::image::stackable_image::StackableImage;

/// A frame as its admission found it: what its source stated of it, and the statistics measured on
/// it before any interpolation. They travel together with the stored frame, as the metadata they
/// come from is dropped for every frame but the first.
#[derive(Debug, Clone)]
pub(crate) struct Admitted {
    pub(crate) stats: FrameStats,
    pub(crate) facts: FrameFacts,
}

impl Admitted {
    /// `image` measured and its facts read, with no check against a set: for a frame that is a
    /// set of its own, or one a test builds.
    pub(crate) fn measure(image: &impl StackableImage) -> Self {
        Self {
            stats: FrameStats::measure(image),
            facts: FrameFacts::of(image),
        }
    }

    /// The frame as the image its drizzle makes of it, which is no sensor frame: a mosaic's color
    /// slots become the channels of their colors, as for the frame its demosaic makes, and
    /// nothing is a mosaic any more.
    pub(crate) const fn into_drizzled(mut self) -> Self {
        self.facts.cfa_type = None;
        self
    }
}
