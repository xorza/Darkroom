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
        self.admit_measured(idx, image, None)
    }

    /// Check frame `idx` and take `measured`, what a decode cache measured of this very image,
    /// rather than measuring it again — again only for a spread `measured` lacks.
    pub(crate) fn admit_measured(
        &self,
        idx: usize,
        image: &impl StackableImage,
        measured: Option<Admitted>,
    ) -> Result<Admitted, AdmissionError> {
        FrameDimensionMismatch::check(idx, self.desc, image.desc())?;
        let facts = FrameFacts::of(image);
        self.check_facts(idx, &facts)?;
        FrameCheck {
            idx,
            cancel: self.cancel,
        }
        .samples(image)?;
        let stats = match measured {
            Some(measured)
                if self.spreads == Spreads::Skipped || measured.stats.spread.is_some() =>
            {
                debug_assert_eq!(measured.facts, facts, "measured of another image");
                measured.stats
            }
            _ => FrameStats::measure_as(image, self.spreads),
        };
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

#[cfg(test)]
mod tests {
    use common::{CancelToken, USize2};

    use crate::frame::admitted::Admitted;
    use crate::ingest::frame_admission::FrameAdmission;
    use crate::internals::images::mono_image;

    /// An admission takes what a decode cache measured of the image: a median the cache recorded
    /// as 7 stays 7 on a frame of 0.5. One that measures spreads, which the record lacks, measures
    /// the frame again, and reads 0.5.
    #[test]
    fn a_measured_frame_is_taken_unless_it_lacks_a_spread() {
        let image = mono_image(USize2::new(8, 8), vec![0.5; 64]);
        let cancel = CancelToken::new();
        let mut measured = Admitted::measure(&image);
        measured.stats.medians[0] = 7.0;
        let median = |admission: FrameAdmission<'_>| {
            admission
                .admit_measured(0, &image, Some(measured.clone()))
                .unwrap()
                .stats
                .medians[0]
        };
        assert_eq!(median(FrameAdmission::new(image.desc(), &cancel)), 7.0);
        assert_eq!(
            median(FrameAdmission::new(image.desc(), &cancel).measuring_spread()),
            0.5
        );
    }
}
