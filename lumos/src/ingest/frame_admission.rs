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
/// before the rest of it decodes. A frame admitted before they were known passes unchecked, so
/// when a parallel run fails at a frame, the intake asks [`Self::first_mismatch`] below it, and
/// names the failure a sequential admission names. The full comparison of the set, in index
/// order, runs once every frame is in (`SetFacts::admit`).
#[derive(Debug)]
pub(crate) struct FrameAdmission<'a> {
    desc: PlanarDesc,
    cancel: &'a CancelToken,
    first_facts: OnceLock<SetFacts>,
    /// Each frame's facts once they passed, by frame index: one writer each.
    facts: Box<[OnceLock<FrameFacts>]>,
    /// Whether each frame's spread is measured too: see [`FrameStats::measure_as`].
    spreads: Spreads,
}

impl<'a> FrameAdmission<'a> {
    /// The admission of `frame_count` frames of `desc`.
    pub(crate) fn new(desc: PlanarDesc, frame_count: usize, cancel: &'a CancelToken) -> Self {
        Self {
            desc,
            cancel,
            first_facts: OnceLock::new(),
            facts: (0..frame_count).map(|_| OnceLock::new()).collect(),
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
        if let Some(first) = self.first_facts.get() {
            first.check(idx, facts)?;
        }
        self.facts[idx]
            .set(facts.clone())
            .expect("each frame's facts are checked once");
        Ok(())
    }

    /// The first frame below `below` whose facts differ from frame 0's, as a sequential
    /// admission names it: a frame admitted before frame 0 published its facts passed unchecked.
    /// `None` when every frame below that passed agrees, or frame 0 has not passed.
    pub(crate) fn first_mismatch(&self, below: usize) -> Option<AdmissionError> {
        let first = SetFacts::of_first(self.facts.first()?.get()?);
        (1..below).find_map(|idx| first.check(idx, self.facts[idx].get()?).err())
    }
}

#[cfg(test)]
mod tests {
    use common::{CancelToken, USize2};
    use imaginarium::{ChannelLayout, PlanarDesc, RasterFormat};

    use crate::frame::admitted::Admitted;
    use crate::image::linear_image::LinearImage;
    use crate::image::metadata::image_provenance::{
        ColorProvenance, DemosaicProvenance, ImageProvenance, RasterTransfer, RowOrder,
        SourceProvenance,
    };
    use crate::ingest::error::AdmissionError;
    use crate::ingest::frame_admission::FrameAdmission;
    use crate::internals::images::mono_image;

    /// A 2×2 frame that stores its rows in `row_order`.
    fn frame(row_order: RowOrder) -> LinearImage {
        let desc = PlanarDesc::new((2, 2), ChannelLayout::Mono);
        let mut image = LinearImage::from_interleaved(desc, vec![1.0; 4]);
        image.metadata.derived.set_provenance(Some(ImageProvenance {
            source: SourceProvenance::Raster {
                format: RasterFormat::Tiff,
                transfer: RasterTransfer::Float,
            },
            color: ColorProvenance::Monochrome,
            clipped: false,
            demosaic: DemosaicProvenance::None,
            row_order,
        }));
        image
    }

    /// The frame a row-order mismatch with frame 0 names.
    fn mismatched(error: &AdmissionError) -> usize {
        match error {
            AdmissionError::RowOrderMismatch {
                idx,
                reference_idx: 0,
                ..
            } => *idx,
            error => panic!("a row-order mismatch with frame 0: {error:?}"),
        }
    }

    /// Frames 1 and 3 store their rows bottom-up against frame 0's top-down, and frame 1 is
    /// admitted before frame 0, as a parallel run can: it passes unchecked, and frame 3 fails.
    /// Below frame 3, the first mismatch is frame 1, which a sequential admission names; below
    /// frame 1 there is none, and before frame 0 passed nothing can be checked.
    #[test]
    fn a_frame_admitted_before_frame_0_is_checked_below_a_failure() {
        let cancel = CancelToken::new();
        let admission =
            FrameAdmission::new(PlanarDesc::new((2, 2), ChannelLayout::Mono), 4, &cancel);
        admission.admit(1, &frame(RowOrder::BottomUp)).unwrap();
        assert!(
            admission.first_mismatch(4).is_none(),
            "frame 0 has not passed"
        );
        admission.admit(0, &frame(RowOrder::TopDown)).unwrap();
        admission.admit(2, &frame(RowOrder::TopDown)).unwrap();
        let failed = admission.admit(3, &frame(RowOrder::BottomUp)).unwrap_err();
        assert_eq!(mismatched(&failed), 3);
        assert_eq!(mismatched(&admission.first_mismatch(3).unwrap()), 1);
        assert!(admission.first_mismatch(1).is_none());
    }

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
        assert_eq!(median(FrameAdmission::new(image.desc(), 1, &cancel)), 7.0);
        assert_eq!(
            median(FrameAdmission::new(image.desc(), 1, &cancel).measuring_spread()),
            0.5
        );
    }
}
