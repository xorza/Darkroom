//! [`FrameAdmission`]: the checks every decoded frame of a run passes, and its statistics.

use std::sync::OnceLock;

use common::CancelToken;

use crate::combine::cache::frame_check::FrameCheck;
use crate::combine::cache::set_facts::SetFacts;
use crate::combine::error::StackError;
use crate::error::FrameDimensionMismatch;
use crate::frame_store::frame_facts::FrameFacts;
use crate::frame_store::frame_stats::FrameStats;
use crate::frame_store::stackable_image::StackableImage;
use crate::io::image::image_dimensions::ImageDimensions;

/// The checks every decoded frame of a run passes before it is parked, in one order — its geometry
/// against the run's, its facts against frame 0's once they are known, then its samples — and the
/// statistics measured on it before any interpolation.
///
/// Frames decode beside each other, so frame 0 publishes its facts when it is admitted, and a
/// later frame is compared with them only when they are already known: a mismatched set stops
/// before the rest of it decodes. The full comparison of the set, in index order, runs once every
/// frame is in (`SetFacts::admit`).
#[derive(Debug)]
pub(crate) struct FrameAdmission<'a> {
    dimensions: ImageDimensions,
    cancel: &'a CancelToken,
    first_facts: OnceLock<SetFacts>,
}

impl<'a> FrameAdmission<'a> {
    pub(crate) const fn new(dimensions: ImageDimensions, cancel: &'a CancelToken) -> Self {
        Self {
            dimensions,
            cancel,
            first_facts: OnceLock::new(),
        }
    }

    pub(crate) const fn dimensions(&self) -> ImageDimensions {
        self.dimensions
    }

    /// Check frame `index` and measure it.
    pub(crate) fn admit(
        &self,
        index: usize,
        image: &impl StackableImage,
    ) -> Result<FrameStats, StackError> {
        FrameDimensionMismatch::check(index, self.dimensions, image.dimensions())?;
        self.check_facts(index, &FrameFacts::of(image))?;
        FrameCheck {
            index,
            cancel: self.cancel,
        }
        .samples(image)?;
        let stats = FrameStats::measure(image);
        if index == 0 {
            self.first_facts
                .set(SetFacts::of_first(&stats.facts))
                .expect("frame 0 is admitted once");
        }
        Ok(stats)
    }

    /// Check the facts of frame `index` against frame 0's, once they are known: for a frame read
    /// back from a kept cache, whose samples were checked when it was written.
    pub(crate) fn check_facts(&self, index: usize, facts: &FrameFacts) -> Result<(), StackError> {
        match self.first_facts.get() {
            Some(first) => first.check(index, facts),
            None => Ok(()),
        }
    }
}
