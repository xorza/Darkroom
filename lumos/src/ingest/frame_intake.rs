//! [`FrameIntake`]: how every entry point takes its frames in, checked and measured, a bounded
//! number at a time.

use std::ops::Range;

use common::par;
use imaginarium::PlanarDesc;

use crate::error::Cancelled;
use crate::frame::admitted::Admitted;
use crate::frame::frame_facts::FrameFacts;
use crate::image::stackable_image::StackableImage;
use crate::ingest::error::AdmissionError;
use crate::ingest::frame_admission::FrameAdmission;
use crate::ingest::ingest_run::IngestRun;
use crate::io::load_ctx::LoadCtx;
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
    ctx: LoadCtx,
    done: StageCounter<'a>,
}

/// What an intake of a run needs besides the run: the frames' shape, how many there are, how many
/// decode at once, and the stage their progress reports under.
#[derive(Debug, Clone, Copy)]
pub(crate) struct IntakePlan {
    pub(crate) desc: PlanarDesc,
    pub(crate) frame_count: usize,
    pub(crate) decode_slots: usize,
    pub(crate) stage: StackingStage,
}

impl<'a> FrameIntake<'a> {
    /// The intake of `plan`'s frames under `run`.
    pub(crate) fn new(plan: IntakePlan, run: &'a IngestRun) -> Self {
        Self {
            admission: FrameAdmission::new(plan.desc, &run.ctx.cancel),
            ctx: run.ctx.for_decode_slots(plan.decode_slots.max(1)),
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
    pub(crate) const fn ctx(&self) -> &LoadCtx {
        &self.ctx
    }

    pub(crate) const fn desc(&self) -> PlanarDesc {
        self.admission.desc()
    }

    /// Check frame `idx` and measure it — see [`FrameAdmission::admit`].
    pub(crate) fn admit(
        &self,
        idx: usize,
        image: &impl StackableImage,
    ) -> Result<Admitted, AdmissionError> {
        self.admission.admit(idx, image)
    }

    /// Check the facts of frame `idx`, read back from a kept cache — see
    /// [`FrameAdmission::check_facts`].
    pub(crate) fn check_facts(&self, idx: usize, facts: &FrameFacts) -> Result<(), AdmissionError> {
        self.admission.check_facts(idx, facts)
    }

    /// Count one more frame done and report it; the count after this one.
    pub(crate) fn complete_one(&self) -> usize {
        self.done.complete_one()
    }

    /// Run `job` over the frames of `idxs`, one slot to each frame in flight and at most
    /// `slots.len()` at once, the results in index order. A cancel stops the frames not yet
    /// started.
    pub(crate) fn map<S, R, E>(
        &self,
        idxs: Range<usize>,
        slots: &mut [S],
        job: impl Fn(&mut S, usize) -> Result<R, E> + Sync,
    ) -> Result<Vec<R>, E>
    where
        S: Send,
        R: Send,
        E: Send + From<Cancelled>,
    {
        let start = idxs.start;
        par::slots(idxs.len(), slots, |slot, offset| {
            Cancelled::check(&self.ctx.cancel)?;
            job(slot, start + offset)
        })
    }

    /// [`Self::map`] for jobs whose workers carry nothing from one frame to the next: at most
    /// `width` frames in flight.
    pub(crate) fn map_at_most<R, E>(
        &self,
        idxs: Range<usize>,
        width: usize,
        job: impl Fn(usize) -> Result<R, E> + Sync,
    ) -> Result<Vec<R>, E>
    where
        R: Send,
        E: Send + From<Cancelled>,
    {
        let start = idxs.start;
        par::slots(idxs.len(), &mut vec![(); width], |(), offset| {
            Cancelled::check(&self.ctx.cancel)?;
            job(start + offset)
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
        par::slots_owned(items, slots, |slot, idx, item| {
            Cancelled::check(&self.ctx.cancel)?;
            job(slot, idx, item)
        })
    }
}
