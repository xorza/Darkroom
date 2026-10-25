//! [`FrameAdmission`]: the checks every decoded frame of a run passes, and its statistics.

use std::sync::OnceLock;

use common::CancelToken;
use imaginarium::PlanarDesc;

use crate::frame::admitted::Admitted;
use crate::frame::frame_facts::FrameFacts;
use crate::frame::frame_stats::{FrameStats, Spreads};
use crate::image::error::FrameDimensionMismatch;
use crate::image::stackable_image::StackableImage;
use crate::ingest::error::AdmissionError;
use crate::ingest::frame_check::FrameCheck;
use crate::ingest::set_facts::SetFacts;

/// The checks every decoded frame of a run passes before it is parked, in one order — its geometry
/// against the run's, its facts against frame 0's once they are known, then its samples — and the
/// statistics measured on it before the warp, with a demosaiced frame's noise taken from its
/// mosaic.
///
/// Frames decode beside each other, so frame 0 publishes its facts when it is admitted, and a
/// later frame is compared with them only when they are already known: a mismatched set stops
/// before the rest of it decodes. The full comparison of the set, in index order, runs once every
/// frame is in (`SetFacts::admit`).
#[derive(Debug)]
pub(crate) struct FrameAdmission<'a> {
    desc: PlanarDesc,
    cancel: &'a CancelToken,
    first_facts: OnceLock<SetFacts>,
    /// Whether each frame's spread is measured too: see [`FrameStats::measure_as`].
    spreads: Spreads,
}

impl<'a> FrameAdmission<'a> {
    pub(crate) const fn new(desc: PlanarDesc, cancel: &'a CancelToken) -> Self {
        Self {
            desc,
            cancel,
            first_facts: OnceLock::new(),
            spreads: Spreads::Skipped,
        }
    }

    /// This admission, measuring each frame's spread beside its other statistics.
    pub(crate) const fn measuring_spread(mut self) -> Self {
        self.spreads = Spreads::Measured;
        self
    }

    pub(crate) const fn desc(&self) -> PlanarDesc {
        self.desc
    }

    /// Check frame `idx` and measure it.
    pub(crate) fn admit(
        &self,
        idx: usize,
        image: &impl StackableImage,
    ) -> Result<Admitted, AdmissionError> {
        FrameDimensionMismatch::check(idx, self.desc, image.desc())?;
        let facts = FrameFacts::of(image);
        self.check_facts(idx, &facts)?;
        FrameCheck {
            idx,
            cancel: self.cancel,
        }
        .samples(image)?;
        let stats = FrameStats::measure_as(image, self.spreads);
        if idx == 0 {
            self.first_facts
                .set(SetFacts::of_first(&facts))
                .expect("frame 0 is admitted once");
        }
        Ok(Admitted { stats, facts })
    }

    /// Check the facts of frame `idx` against frame 0's, once they are known: for a frame read
    /// back from a kept cache, whose samples were checked when it was written.
    pub(crate) fn check_facts(&self, idx: usize, facts: &FrameFacts) -> Result<(), AdmissionError> {
        match self.first_facts.get() {
            Some(first) => first.check(idx, facts),
            None => Ok(()),
        }
    }
}
