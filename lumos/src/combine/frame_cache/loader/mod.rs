//! Tier selection, frame loading, and persistent cache sidecars.

use std::path::Path;

use crate::calibration::error::SubtractionError;
use crate::combine::error::StackError;
use crate::combine::frame_cache::core::CacheCore;
use crate::combine::stack_config::StackConfig;
use crate::error::FrameDimensionMismatch;
use crate::frame::frame_peek::FramePeek;
use crate::frame::frame_quality::FrameQuality;
use crate::frame::stackable_image::StackableImage;
use crate::frame_store::decode_cache::DecodeCache;
use crate::frame_store::decode_cache::cached_source::CachedSource;
use crate::frame_store::error::FrameStoreError;
use crate::frame_store::stored_frame::StoredFrame;
use crate::frame_store::tier::Tier;
use crate::image::image_metadata::ImageMetadata;
use crate::ingest::frame_check::FrameCheck;
use crate::ingest::frame_intake::{FrameIntake, IntakePlan};
use crate::ingest::frame_step::FrameStep;
use crate::ingest::ingest_run::IngestRun;
use crate::io::error::ImageError;
use crate::io::load_context::LoadContext;
use crate::memory::run_plan::RunPlan;
use crate::progress::progress_monitor::ProgressMonitor;
use crate::progress::stacking_progress::StackingStage;

/// [`load_tiered`] output: the loaded frames plus the assembled [`CacheCore`].
#[derive(Debug)]
pub(super) struct LoadedCache {
    pub(super) frames: Vec<StoredFrame>,
    pub(super) core: CacheCore,
}

/// The stage of [`FramePeek::stages`] that decodes the frames.
const DECODE: usize = 0;

/// Load every frame into the tier its set fits, each through `step` when given.
///
/// A prepared frame is not what its source decodes to, so on the disk tier it is spilled for this
/// run alone, never committed to the kept cache nor read back from it: a later run with another
/// step, or none, would otherwise reuse it.
pub(super) fn load_tiered<I: StackableImage>(
    paths: &[&Path],
    config: &StackConfig,
    run: IngestRun,
    step: Option<&dyn FrameStep<I, Error = SubtractionError>>,
) -> Result<LoadedCache, StackError> {
    debug_assert!(!paths.is_empty(), "`combine_cached` refuses an empty set");
    let first_path = paths[0];

    // Dimensions drive the in-memory-vs-disk tier decision. Peek the header without a decode when
    // the format allows it, so the in-memory path can decode every frame in parallel rather than
    // decoding frame 0 serially first; otherwise decode frame 0 and reuse it below.
    let (peek, early) = if let Some(peek) = I::peek(first_path, &run.context) {
        (peek, None)
    } else {
        let source = CachedSource::of(first_path, I::DECODER, &run.context);
        let mut image = load_image::<I>(first_path, &run.context)?;
        if let Some(step) = step {
            step.apply(0, &mut image)?;
        }
        (
            FramePeek::of_decoded(&image),
            Some(Decoded { image, source }),
        )
    };
    let dimensions = peek.dimensions;
    // Frames of a set share a source, so frame 0 stands for all of them — including whether they
    // carry the two quality planes a masked frame does, which `FramePeek::resident_bytes` charges
    // for.
    let budget = run.budget;
    let plan = RunPlan::of(
        paths.len(),
        &peek.stages(paths.len(), config, budget.workers()),
        budget,
    );
    tracing::info!(
        frame_count = paths.len(),
        sample_count = dimensions.sample_count(),
        budget_mb = budget.usable() / (1024 * 1024),
        resident = plan.resident,
        "Image cache storage decision"
    );

    // A decode-level memo: looked up before any decode on every tier, its planes mapped from the
    // files whatever the tier, since a mapped plane reads as a resident one does. A frame through
    // a step is not what its source decodes to, so a stepped run neither reads nor writes it.
    let kept = (run.keep_cache && step.is_none())
        .then(|| DecodeCache::open(&run.cache_dir))
        .transpose()?;
    let tier = Tier::for_plan(
        &plan,
        budget,
        if kept.is_some() {
            0
        } else {
            (peek.resident_bytes() as u64).saturating_mul(paths.len() as u64)
        },
        &run.cache_dir,
    )?;
    if let Some(kept) = &kept {
        let missing = kept.missing::<I>(paths, &run.context) as u64;
        kept.ensure_space(missing.saturating_mul(peek.resident_bytes() as u64))?;
    }
    let intake = FrameIntake::new(
        IntakePlan {
            dimensions,
            frame_count: paths.len(),
            decode_slots: plan.width(DECODE),
            stage: StackingStage::Loading,
        },
        &run,
    );
    let load = FrameLoad {
        paths,
        tier: &tier,
        kept: kept.as_ref(),
        intake: &intake,
        step,
    };

    // Frame 0 carries the stack metadata, which a kept frame keeps with it. Frame 0 decoded for
    // the plan is taken in first; every other frame in parallel.
    let first = early.map(|early| load.frame(0, Some(early))).transpose()?;
    let start = usize::from(first.is_some());
    let mut slots = vec![(); plan.width(DECODE)];
    let rest = intake.map(start..paths.len(), &mut slots, |(), index| {
        load.frame(index, None)
    })?;
    let mut metadata = None;
    let mut frames = Vec::with_capacity(paths.len());
    for loaded in first.into_iter().chain(rest) {
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
            monitor: ProgressMonitor::new(run.progress, run.context.cancel),
        },
    })
}

fn load_image<I: StackableImage>(path: &Path, context: &LoadContext) -> Result<I, StackError> {
    match I::load(path, context) {
        Ok(image) => Ok(image),
        // Cancellation is the run stopping, not this file failing, so it leaves the load error
        // behind and reports as the stack's own.
        Err(ImageError::Cancelled { .. }) => Err(StackError::Cancelled),
        Err(source) => Err(StackError::ImageLoad(source)),
    }
}

/// A decoded frame and its source as it was before the decode, which only the kept cache reads —
/// for its source-change check — so only the kept cache fails on it.
#[derive(Debug)]
struct Decoded<I> {
    image: I,
    source: Result<CachedSource, FrameStoreError>,
}

/// A frame in its tier, with the stack metadata when it is frame 0.
#[derive(Debug)]
struct LoadedFrame {
    frame: StoredFrame,
    metadata: Option<ImageMetadata>,
}

/// One frame set's load into its tier: where each frame goes, and how it is taken in.
#[derive(Debug)]
struct FrameLoad<'a, I> {
    paths: &'a [&'a Path],
    tier: &'a Tier,
    /// The decode cache, when `keep_cache` asked for one.
    kept: Option<&'a DecodeCache>,
    intake: &'a FrameIntake<'a>,
    step: Option<&'a dyn FrameStep<I, Error = SubtractionError>>,
}

impl<I: StackableImage> FrameLoad<'_, I> {
    /// Frame `index` decoded, through the step when there is one, with its source as it was
    /// before the decode.
    fn decode(&self, index: usize) -> Result<Decoded<I>, StackError> {
        let path = self.paths[index];
        let source = CachedSource::of(path, I::DECODER, self.intake.context());
        let mut image = load_image::<I>(path, self.intake.context())?;
        if let Some(step) = self.step {
            step.apply(index, &mut image)?;
        }
        Ok(Decoded { image, source })
    }

    /// Frame `index` in its tier: with a decode cache, the planes it holds committed for this
    /// source and decoder, or a decode written there and committed; without one, a decode the tier
    /// keeps.
    ///
    /// `decoded` is a frame already decoded, which is stored rather than looked up: the decode a
    /// lookup would save is already spent. A frame through `step` is not what its source decodes
    /// to, so it goes to the tier whatever the cache.
    fn frame(&self, index: usize, decoded: Option<Decoded<I>>) -> Result<LoadedFrame, StackError> {
        if let Some(kept) = self.kept {
            return self.kept_frame(kept, index, decoded);
        }
        let image = match decoded {
            Some(decoded) => decoded.image,
            None => self.decode(index)?.image,
        };
        let stats = self.intake.admit(index, &image)?;
        let metadata = (index == 0).then(|| image.metadata().clone());
        let quality = FrameQuality::for_unwarped(&image);
        let frame = self.tier.store(image, quality, stats)?;
        self.intake.complete_one();
        Ok(LoadedFrame { frame, metadata })
    }

    /// Frame `index` through the decode cache: the planes committed there for its source as it is
    /// now, or a decode written there and committed. The source's identity is read before the
    /// decode and again after it, and a frame whose source changed in between is refused rather
    /// than cached under the identity it had before.
    fn kept_frame(
        &self,
        kept: &DecodeCache,
        index: usize,
        decoded: Option<Decoded<I>>,
    ) -> Result<LoadedFrame, StackError> {
        let path = self.paths[index];
        let dimensions = self.intake.dimensions();
        let context = self.intake.context();
        let (source, decoded_image) = match decoded {
            Some(Decoded { image, source }) => (source?, Some(image)),
            None => (CachedSource::of(path, I::DECODER, context)?, None),
        };

        if decoded_image.is_none()
            && let Some(reused) = kept.reuse(&source)?
        {
            tracing::debug!(source = %path.display(), "Reusing existing cache files");
            FrameDimensionMismatch::check(index, dimensions, reused.dimensions)?;
            let frame = reused.frame;
            self.intake.check_facts(index, &frame.source_stats.facts)?;
            let check = FrameCheck {
                index,
                cancel: &context.cancel,
            };
            check.stored_samples(&frame.channels, dimensions.pixel_count())?;
            // Mapped from disk, so held to the pairing the combine divides by like any other planes.
            check.stored_quality(&frame, dimensions)?;
            self.intake.complete_one();
            return Ok(LoadedFrame {
                frame,
                metadata: (index == 0).then_some(reused.metadata),
            });
        }

        let image = match decoded_image {
            Some(image) => image,
            None => load_image::<I>(path, context)?,
        };
        let stats = self.intake.admit(index, &image)?;
        let metadata = (index == 0).then(|| image.metadata().clone());
        let frame = kept.keep(&source, &image, stats)?;
        self.intake.complete_one();
        Ok(LoadedFrame { frame, metadata })
    }
}

#[cfg(test)]
mod tests;
