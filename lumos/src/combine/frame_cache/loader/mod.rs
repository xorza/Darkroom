//! StorageTier selection, frame loading, and persistent cache sidecars.

use std::path::Path;

use crate::combine::error::StackError;
use crate::combine::frame_cache::core::CacheCore;
use crate::combine::frame_cache::frame_step::{FrameStep, SteppedPaths};
use crate::combine::frame_read::slots::Slots;
use crate::combine::normalization::planned_normalization::SetCoverage;
use crate::combine::stack::planned_combine::{PlannedCombine, PlannedFrames};
use crate::error::Cancelled;
use crate::frame::frame_peek::FramePeek;
use crate::frame::frame_quality::FrameQuality;
use crate::frame_store::decode_cache::DecodeCache;
use crate::frame_store::decode_cache::kept_frame::KeptFrame;
use crate::frame_store::file_set::{FileSet, SetPeek, Taken};
use crate::frame_store::frame_buffers::NewFrame;
use crate::frame_store::run_storage::RunWrites;
use crate::frame_store::storage_tier::StorageTier;
use crate::frame_store::stored_frame::{SetKind, StoredFrame};
use crate::frame_store::stored_layout::StoredLayout;
use crate::image::error::FrameDimensionMismatch;
use crate::image::image_metadata::ImageMetadata;
use crate::image::stackable_image::StackableImage;
use crate::ingest::frame_check::FrameCheck;
use crate::ingest::frame_intake::{FrameIntake, IntakePlan};
use crate::ingest::ingest_run::IngestRun;
use crate::io::decoder::Decoder;
use crate::io::load_ctx::LoadCtx;
use crate::memory::run_plan::RunPlan;
use crate::memory::stage::{Stage, Units};
use crate::progress::stacking_progress::StackingStage;

/// [`load_tiered`] output: the loaded frames plus the assembled [`CacheCore`].
#[derive(Debug)]
pub(super) struct LoadedCache {
    pub(super) frames: Vec<StoredFrame>,
    pub(super) core: CacheCore,
}

/// The stage of [`stages`] that decodes the frames.
const DECODE: usize = 0;

/// The stages of a run that decodes `frame_count` frames such as `peek` reads straight into
/// `combine`, on `workers`: each frame decoded and measured, beside what the peek holds for its
/// frame's decode; the normalization; the combine, whose rows read every frame as it decoded; each
/// beside the source gains the frames share. A frame may carry a mask its decode alone reveals, so
/// the normalization is charged as over a set that shares only some pixels, which holds more than
/// one every frame covers.
fn stages(
    combine: PlannedCombine,
    peek: SetPeek,
    frame_count: usize,
    workers: usize,
) -> [Stage; 3] {
    let frame = FramePeek::from(peek.info);
    let dimensions = frame.dimensions;
    let decoded = frame.footprint(workers);
    let [normalize, combine] = combine.stages(
        PlannedFrames {
            count: frame_count,
            dimensions,
            layout: StoredLayout::decoded(dimensions),
            slots: Slots::new(frame.mosaic, dimensions.channels()),
            coverage: SetCoverage::Common,
            workers,
        },
        decoded.kept,
    );
    [
        Stage {
            kept: decoded.kept,
            held: peek.held,
            in_flight: decoded.transient(),
            units: Units::Frames,
        },
        normalize,
        combine,
    ]
    .map(|stage| Stage {
        held: stage.held + peek.gains,
        ..stage
    })
}

/// Load every frame into the tier its set fits, each through its step.
///
/// A frame through a step is not what its source decodes to, so its decode is read through the
/// decode cache and the stepped frame goes to the tier, spilled for this run alone when the tier
/// spills: a later run with another step, or none, would otherwise reuse it.
pub(super) fn load_tiered<D, S, E>(
    frames: SteppedPaths<'_, &Path, S>,
    combine: PlannedCombine,
    run: IngestRun,
) -> Result<LoadedCache, E>
where
    D: Decoder,
    S: FrameStep<D::Image>,
    E: From<StackError> + From<S::Error> + From<Cancelled> + Send,
{
    let SteppedPaths { paths, step } = frames;
    debug_assert!(!paths.is_empty(), "`combine_cached` refuses an empty set");

    // A decode-level memo: looked up before any decode on every tier. An unstepped frame is its
    // kept planes, mapped from the files whatever the tier, since a mapped plane reads as a
    // resident one does.
    let files = FileSet::<_, D>::new(paths, &run).map_err(StackError::from)?;
    let kept = files.storage().kept().filter(|_| !step.changes_frames());

    // Dimensions drive the in-memory-vs-disk tier decision. A header answers without a decode
    // where the format allows it, so the in-memory path decodes every frame in parallel; otherwise
    // one frame is decoded first, and its decode is the one its turn takes.
    let peek = stack(files.peek(&run.ctx))?;
    let dimensions = peek.info.dimensions;
    // Frames of a set share a source, so one frame stands for all of them — including whether
    // they carry the two quality planes a masked frame does, which `FramePeek::resident_bytes`
    // charges for.
    let budget = run.budget;
    let plan = RunPlan::of(
        paths.len(),
        &stages(combine, peek, paths.len(), budget.workers()),
        budget,
    );
    tracing::info!(
        frame_count = paths.len(),
        sample_count = dimensions.sample_count(),
        budget_mb = budget.usable() / (1024 * 1024),
        resident = plan.resident,
        "Image cache storage decision"
    );

    // A kept frame writes its gain too, a spilled one only its planes: its gain is the run's.
    let frame_bytes = dimensions.decoded_bytes() as u64;
    let writes = RunWrites {
        kept: (files.uncached() as u64)
            .saturating_mul(frame_bytes.saturating_add(peek.info.gain_bytes as u64)),
        spilled: if kept.is_some() {
            0
        } else {
            frame_bytes.saturating_mul(paths.len() as u64)
        },
    };
    let tier = stack(files.storage().tier(&plan, writes))?.holding_beside(peek.gains);
    let intake = FrameIntake::new(
        IntakePlan {
            dimensions,
            frame_count: paths.len(),
            decode_slots: plan.width(DECODE),
            stage: StackingStage::Loading,
        },
        &run,
    );
    let store = FrameStore {
        files: &files,
        kept,
        step,
        ctx: &run.ctx,
        tier: &tier,
        intake: &intake,
    };

    // Frame 0 carries the stack metadata, which a kept frame keeps with it.
    let loaded = intake.map_at_most(0..paths.len(), plan.width(DECODE), |idx| {
        store.frame::<E>(idx)
    })?;
    let mut metadata = None;
    let mut frames = Vec::with_capacity(paths.len());
    for loaded in loaded {
        metadata = metadata.or(loaded.metadata);
        frames.push(loaded.frame);
    }
    tracing::info!(
        frames = frames.len(),
        spilled = tier.spills(),
        "Loaded the frames into their tier"
    );

    Ok(LoadedCache {
        frames,
        core: CacheCore {
            tier,
            dimensions,
            metadata: metadata.expect("frame 0 provides metadata"),
            monitor: run.into_monitor(),
        },
    })
}

/// A frame in its tier, with the stack metadata when it is frame 0.
#[derive(Debug)]
struct LoadedFrame {
    frame: StoredFrame,
    metadata: Option<ImageMetadata>,
}

/// One frame set's load into its tier: where each frame is taken from, decoded through its step or
/// from the decode cache, where it goes, and how it is taken in.
#[derive(Debug)]
struct FrameStore<'a, D: Decoder, S> {
    files: &'a FileSet<'a, &'a Path, D>,
    /// The decode cache whose planes are the frames: when `keep_cache` asked for one and the step
    /// changes nothing.
    kept: Option<&'a DecodeCache>,
    step: &'a S,
    ctx: &'a LoadCtx,
    tier: &'a StorageTier,
    intake: &'a FrameIntake<'a>,
}

impl<D: Decoder, S: FrameStep<D::Image>> FrameStore<'_, D, S> {
    /// Frame `idx` in its tier: with a decode cache, the planes it holds committed for this
    /// source and decoder, or a decode written there and committed; without one, a decode through
    /// the step, which the tier keeps. A frame through the step is not what its source decodes
    /// to, so it goes to the tier whatever the cache.
    fn frame<E: From<StackError> + From<S::Error>>(&self, idx: usize) -> Result<LoadedFrame, E> {
        if let Some(kept) = self.kept {
            return Ok(self.kept_frame(kept, idx)?);
        }
        let mut image = stack(self.files.decode(idx, self.ctx))?.image;
        self.step.apply(idx, &mut image)?;
        let admitted = stack(self.intake.admit(idx, &image))?;
        let metadata = (idx == 0).then(|| image.metadata().clone());
        let quality = FrameQuality::for_unwarped(image.flag_counts());
        let frame = stack(self.tier.store(
            NewFrame { image, quality },
            admitted,
            SetKind::Unwarped,
        ))?
        .frame;
        self.intake.complete_one();
        Ok(LoadedFrame { frame, metadata })
    }

    /// Frame `idx` through the decode cache: the planes committed there for its source as it is
    /// now, or a decode written there and committed. The source's identity is read before the
    /// decode and again after it, and a frame whose source changed in between is refused rather
    /// than cached under the identity it had before.
    fn kept_frame(&self, kept: &DecodeCache, idx: usize) -> Result<LoadedFrame, StackError> {
        debug_assert!(
            !self.step.changes_frames(),
            "a stepped run neither reads nor writes the decode cache"
        );
        let (image, source) = match self.files.take(idx, self.ctx)? {
            Taken::Kept(reused) => {
                tracing::debug!(source = %self.files.path(idx).display(), "Reusing existing cache files");
                return self.reused(idx, *reused);
            }
            Taken::Decoded { image, source } => (image, source),
        };
        let admitted = self.intake.admit(idx, &image)?;
        let written = kept.keep(&source, &image, admitted)?;
        drop(image);
        let metadata = (idx == 0).then(|| written.image.metadata.clone());
        let frame = written.into_frame();
        self.intake.complete_one();
        Ok(LoadedFrame { frame, metadata })
    }

    /// Frame `idx` as a run before kept it, held to the set's shape and facts and to the planes'
    /// checks, as a decode is on admission.
    fn reused(&self, idx: usize, reused: KeptFrame) -> Result<LoadedFrame, StackError> {
        let dimensions = self.intake.dimensions();
        FrameDimensionMismatch::check(idx, dimensions, reused.dimensions())?;
        self.intake.check_facts(idx, &reused.admitted.facts)?;
        let metadata = (idx == 0).then(|| reused.image.metadata.clone());
        let frame = reused.into_frame();
        let check = FrameCheck {
            idx,
            cancel: &self.ctx.cancel,
        };
        check.stored_samples(&frame.channels, dimensions.pixel_count())?;
        // Mapped from disk, so held to the pairing the combine divides by like any other planes.
        check.stored_quality(&frame, dimensions)?;
        self.intake.complete_one();
        Ok(LoadedFrame { frame, metadata })
    }
}

/// `result` with its error as the stack's: what a load fails with, but for its step.
fn stack<T>(result: Result<T, impl Into<StackError>>) -> Result<T, StackError> {
    result.map_err(Into::into)
}

#[cfg(test)]
mod tests;
