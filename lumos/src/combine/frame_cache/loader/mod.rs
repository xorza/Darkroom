//! Tier selection, frame loading, and persistent cache sidecars.

use std::marker::PhantomData;
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
use crate::frame_store::decode_cache::cached_source::CachedSource;
use crate::frame_store::decode_cache::kept_frame::KeptFrame;
use crate::frame_store::run_storage::{RunStorage, RunWrites};
use crate::frame_store::stored_frame::StoredFrame;
use crate::frame_store::stored_layout::StoredLayout;
use crate::frame_store::tier::Tier;
use crate::image::cfa::cfa_type::CfaType;
use crate::image::error::FrameDimensionMismatch;
use crate::image::image_metadata::ImageMetadata;
use crate::image::stackable_image::StackableImage;
use crate::ingest::frame_check::FrameCheck;
use crate::ingest::frame_intake::{FrameIntake, IntakePlan};
use crate::ingest::ingest_run::IngestRun;
use crate::io::decoder::Decoder;
use crate::io::load_context::LoadContext;
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

/// The stages of a run that decodes `frame_count` frames such as `peek` straight into `combine`,
/// on `workers`: each frame decoded and measured; the normalization; the combine, whose rows read
/// every frame as it decoded. A frame may carry a mask its decode alone reveals, so the
/// normalization is charged as over a set that shares only some pixels, which holds more than one
/// every frame covers.
fn stages(
    combine: PlannedCombine,
    peek: FramePeek,
    frame_count: usize,
    workers: usize,
) -> [Stage; 3] {
    let dimensions = peek.dimensions;
    let decoded = peek.footprint(workers);
    let [normalize, combine] = combine.stages(
        PlannedFrames {
            count: frame_count,
            dimensions,
            layout: StoredLayout::decoded(dimensions),
            slots: Slots::new(peek.mosaic, dimensions.channels()),
            coverage: SetCoverage::Common,
            workers,
        },
        decoded.kept,
    );
    [
        Stage {
            kept: decoded.kept,
            held: 0,
            in_flight: decoded.transient(),
            units: Units::Frames,
        },
        normalize,
        combine,
    ]
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
    let first_path = paths[0];

    // A decode-level memo: looked up before any decode on every tier. An unstepped frame is its
    // kept planes, mapped from the files whatever the tier, since a mapped plane reads as a
    // resident one does.
    let storage = stack(RunStorage::decoding(&run))?;
    let load = FrameLoad::<D, _> {
        paths,
        storage: &storage,
        kept: storage.kept().filter(|_| !step.changes_frames()),
        context: &run.context,
        step,
        decoder: PhantomData,
    };

    // Dimensions drive the in-memory-vs-disk tier decision. Peek the header without a decode when
    // the format allows it, so the in-memory path can decode every frame in parallel rather than
    // decoding frame 0 serially first; otherwise take frame 0 kept from a run before, or decode it,
    // and store it below.
    let (peek, early) = if let Some(info) = D::peek(first_path, &run.context) {
        (FramePeek::from(info), None)
    } else {
        let early = load.early::<E>()?;
        (early.peek(), Some(early))
    };
    let dimensions = peek.dimensions;
    // Frames of a set share a source, so frame 0 stands for all of them — including whether they
    // carry the two quality planes a masked frame does, which `FramePeek::resident_bytes` charges
    // for.
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

    let frame_bytes = peek.resident_bytes() as u64;
    let writes = RunWrites {
        kept: (storage.uncached::<D>(paths, &run.context) as u64).saturating_mul(frame_bytes),
        spilled: if load.kept.is_some() {
            0
        } else {
            frame_bytes.saturating_mul(paths.len() as u64)
        },
    };
    let tier = stack(storage.tier(&plan, writes))?;
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
        load: &load,
        tier: &tier,
        intake: &intake,
    };

    // Frame 0 carries the stack metadata, which a kept frame keeps with it. Frame 0 taken for the
    // plan is stored first; every other frame in parallel.
    let first = early
        .map(|early| store.frame::<E>(0, Some(early)))
        .transpose()?;
    let start = usize::from(first.is_some());
    let mut slots = vec![(); plan.width(DECODE)];
    let rest = intake.map(start..paths.len(), &mut slots, |(), index| {
        store.frame::<E>(index, None)
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
            monitor: run.into_monitor(),
        },
    })
}

/// A decoded frame, with its source as it was before the decode when a decode cache reads it.
#[derive(Debug)]
struct Decoded<I> {
    image: I,
    source: Option<CachedSource>,
}

/// Frame 0 taken before the plan: decoded, or kept by a run before.
#[derive(Debug)]
enum Early<I> {
    Decoded(Decoded<I>),
    Kept(Box<KeptFrame>),
}

impl<I: StackableImage> Early<I> {
    /// What the frame tells the plan. A kept frame was decoded by a run before, so what its decoder
    /// held is not known, as for a frame decoded here.
    fn peek(&self) -> FramePeek {
        match self {
            Self::Decoded(decoded) => FramePeek::of_decoded(&decoded.image),
            Self::Kept(kept) => FramePeek {
                dimensions: kept.dimensions(),
                decoder_bytes: 0,
                mosaic: kept.admitted.facts.cfa_type.filter(CfaType::is_mosaic),
            },
        }
    }
}

/// A frame in its tier, with the stack metadata when it is frame 0.
#[derive(Debug)]
struct LoadedFrame {
    frame: StoredFrame,
    metadata: Option<ImageMetadata>,
}

/// How one frame set's frames are taken: decoded through its step, or from the decode cache.
#[derive(Debug)]
struct FrameLoad<'a, D, S> {
    paths: &'a [&'a Path],
    storage: &'a RunStorage,
    /// The decode cache whose planes are the frames: when `keep_cache` asked for one and the step
    /// changes nothing.
    kept: Option<&'a DecodeCache>,
    context: &'a LoadContext,
    step: &'a S,
    decoder: PhantomData<fn() -> D>,
}

impl<D: Decoder, S: FrameStep<D::Image>> FrameLoad<'_, D, S> {
    /// Frame `index` decoded, through the step when there is one: with its source as it was
    /// before the decode when its planes are kept, through the storage otherwise.
    fn decode<E: From<StackError> + From<S::Error>>(
        &self,
        index: usize,
    ) -> Result<Decoded<D::Image>, E> {
        let path = self.paths[index];
        let (mut image, source) = match self.kept {
            Some(_) => {
                let source = stack(CachedSource::of(path, D::KIND, self.context))?;
                (stack(D::load(path, self.context))?, Some(source))
            }
            None => (stack(self.storage.decode::<D>(path, self.context))?, None),
        };
        self.step.apply(index, &mut image)?;
        Ok(Decoded { image, source })
    }

    /// Frame 0 before the plan: kept by a run before when the decode cache holds it, decoded
    /// otherwise.
    fn early<E: From<StackError> + From<S::Error>>(&self) -> Result<Early<D::Image>, E> {
        if let Some(kept) = self.kept {
            let source = stack(CachedSource::of(self.paths[0], D::KIND, self.context))?;
            if let Some(reused) = kept.reuse(&source) {
                return Ok(Early::Kept(Box::new(reused)));
            }
            let mut image = stack(D::load(self.paths[0], self.context))?;
            self.step.apply(0, &mut image)?;
            return Ok(Early::Decoded(Decoded {
                image,
                source: Some(source),
            }));
        }
        Ok(Early::Decoded(self.decode::<E>(0)?))
    }
}

/// One frame set's load into its tier: how each frame is taken, where it goes, and how it is
/// taken in.
#[derive(Debug)]
struct FrameStore<'a, D, S> {
    load: &'a FrameLoad<'a, D, S>,
    tier: &'a Tier,
    intake: &'a FrameIntake<'a>,
}

impl<D: Decoder, S: FrameStep<D::Image>> FrameStore<'_, D, S> {
    /// Frame `index` in its tier: with a decode cache, the planes it holds committed for this
    /// source and decoder, or a decode written there and committed; without one, a decode the tier
    /// keeps.
    ///
    /// `early` is frame 0 already taken for the plan, which is stored rather than taken again. A
    /// frame through `step` is not what its source decodes to, so it goes to the tier whatever the
    /// cache.
    fn frame<E: From<StackError> + From<S::Error>>(
        &self,
        index: usize,
        early: Option<Early<D::Image>>,
    ) -> Result<LoadedFrame, E> {
        if let Some(kept) = self.load.kept {
            return Ok(self.kept_frame(kept, index, early)?);
        }
        let image = match early {
            Some(Early::Decoded(decoded)) => decoded.image,
            Some(Early::Kept(_)) => unreachable!("a frame is kept only with a decode cache"),
            None => self.load.decode::<E>(index)?.image,
        };
        let admitted = stack(self.intake.admit(index, &image))?;
        let metadata = (index == 0).then(|| image.metadata().clone());
        let quality = FrameQuality::for_unwarped(&image);
        let frame = stack(self.tier.store(image, quality, admitted))?;
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
        early: Option<Early<D::Image>>,
    ) -> Result<LoadedFrame, StackError> {
        debug_assert!(
            !self.load.step.changes_frames(),
            "a stepped run neither reads nor writes the decode cache"
        );
        let path = self.load.paths[index];
        let context = self.load.context;
        let (source, image) = match early {
            Some(Early::Kept(reused)) => return self.reused(index, *reused),
            Some(Early::Decoded(Decoded { image, source })) => (
                source.expect("a decode with a decode cache reads its source"),
                image,
            ),
            None => {
                let source = CachedSource::of(path, D::KIND, context)?;
                if let Some(reused) = kept.reuse(&source) {
                    tracing::debug!(source = %path.display(), "Reusing existing cache files");
                    return self.reused(index, reused);
                }
                (source, D::load(path, context)?)
            }
        };
        let admitted = self.intake.admit(index, &image)?;
        let written = kept.keep(&source, &image, admitted)?;
        drop(image);
        let metadata = (index == 0).then(|| written.image.metadata.clone());
        let frame = written.into_frame();
        self.intake.complete_one();
        Ok(LoadedFrame { frame, metadata })
    }

    /// Frame `index` as a run before kept it, held to the set's shape and facts and to the planes'
    /// checks, as a decode is on admission.
    fn reused(&self, index: usize, reused: KeptFrame) -> Result<LoadedFrame, StackError> {
        let dimensions = self.intake.dimensions();
        FrameDimensionMismatch::check(index, dimensions, reused.dimensions())?;
        self.intake.check_facts(index, &reused.admitted.facts)?;
        let metadata = (index == 0).then(|| reused.image.metadata.clone());
        let frame = reused.into_frame();
        let check = FrameCheck {
            index,
            cancel: &self.load.context.cancel,
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
