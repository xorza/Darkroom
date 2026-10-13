//! [`FrameIntake`]: how every entry point takes its frames in, checked and measured, a bounded
//! number at a time.

use std::ops::Range;

use crate::concurrency;
use crate::error::Cancelled;
use crate::frame::frame_facts::FrameFacts;
use crate::frame::frame_stats::FrameStats;
use crate::frame::stackable_image::StackableImage;
use crate::image::image_dimensions::ImageDimensions;
use crate::ingest::error::AdmissionError;
use crate::ingest::frame_admission::FrameAdmission;
use crate::ingest::ingest_run::IngestRun;
use crate::io::load_context::LoadContext;
use crate::progress::stacking_progress::StackingStage;
use crate::progress::stage_counter::StageCounter;

/// One run's intake of its frames: the checks every frame passes and the statistics measured on
/// it ([`FrameAdmission`]), the decode context with the machine's threads divided among the
/// frames that decode at once, the stage's progress, and the loop that takes them, a bounded
/// number at a time, stopping at a cancel.
///
/// What each entry point does with an admitted frame — store it, detect its stars and park it,
/// drizzle it — is its own; how a frame is taken in is this, for all of them.
#[derive(Debug)]
pub(crate) struct FrameIntake<'a> {
    admission: FrameAdmission<'a>,
    context: LoadContext,
    done: StageCounter<'a>,
}

/// What an intake of a run needs besides the run: the frames' shape, how many there are, how many
/// decode at once, and the stage their progress reports under.
#[derive(Debug, Clone, Copy)]
pub(crate) struct IntakePlan {
    pub(crate) dimensions: ImageDimensions,
    pub(crate) frame_count: usize,
    pub(crate) decode_slots: usize,
    pub(crate) stage: StackingStage,
}

impl<'a> FrameIntake<'a> {
    /// The intake of `plan`'s frames under `run`.
    pub(crate) fn new(plan: IntakePlan, run: &'a IngestRun) -> Self {
        Self {
            admission: FrameAdmission::new(plan.dimensions, &run.context.cancel),
            context: run.context.for_decode_slots(plan.decode_slots.max(1)),
            done: StageCounter::new(&run.progress, plan.stage, plan.frame_count),
        }
    }

    /// This intake, measuring each frame's spread beside its other statistics, for a
    /// normalization that scales frames one by one.
    pub(crate) fn measuring_spread(self) -> Self {
        Self {
            admission: self.admission.measuring_spread(),
            ..self
        }
    }

    /// The decode context of a frame decoding beside the others.
    pub(crate) const fn context(&self) -> &LoadContext {
        &self.context
    }

    pub(crate) const fn dimensions(&self) -> ImageDimensions {
        self.admission.dimensions()
    }

    /// Check frame `index` and measure it — see [`FrameAdmission::admit`].
    pub(crate) fn admit(
        &self,
        index: usize,
        image: &impl StackableImage,
    ) -> Result<FrameStats, AdmissionError> {
        self.admission.admit(index, image)
    }

    /// Check the facts of frame `index`, read back from a kept cache — see
    /// [`FrameAdmission::check_facts`].
    pub(crate) fn check_facts(
        &self,
        index: usize,
        facts: &FrameFacts,
    ) -> Result<(), AdmissionError> {
        self.admission.check_facts(index, facts)
    }

    /// Count one more frame done and report it; the count after this one.
    pub(crate) fn complete_one(&self) -> usize {
        self.done.complete_one()
    }

    /// Run `job` over the frames of `indices`, one slot to each frame in flight and at most
    /// `slots.len()` at once, the results in index order. A cancel stops the frames not yet
    /// started.
    pub(crate) fn map<S, R, E>(
        &self,
        indices: Range<usize>,
        slots: &mut [S],
        job: impl Fn(&mut S, usize) -> Result<R, E> + Sync,
    ) -> Result<Vec<R>, E>
    where
        S: Send,
        R: Send,
        E: Send + From<Cancelled>,
    {
        let start = indices.start;
        concurrency::try_par_map_bounded(indices.len(), slots, |slot, offset| {
            Cancelled::check(&self.context.cancel)?;
            job(slot, start + offset)
        })
    }

    /// [`Self::map`] over `items` the caller hands over, each moved into its job, so a frame's
    /// input is freed as soon as its job is done with it.
    pub(crate) fn map_owned<T, S, R, E>(
        &self,
        items: Vec<T>,
        slots: &mut [S],
        job: impl Fn(&mut S, usize, T) -> Result<R, E> + Sync,
    ) -> Result<Vec<R>, E>
    where
        T: Send,
        S: Send,
        R: Send,
        E: Send + From<Cancelled>,
    {
        concurrency::try_par_map_bounded_owned(items, slots, |slot, index, item| {
            Cancelled::check(&self.context.cancel)?;
            job(slot, index, item)
        })
    }
}
