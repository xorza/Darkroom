//! Tier selection, frame loading, and persistent cache sidecars.

use std::fs;
use std::path::{Path, PathBuf};

use common::FileIdentity;
use imaginarium::Buffer2;

use crate::combine::config::StackConfig;
use crate::combine::error::StackError;
use crate::concurrency;
use crate::frame_store::cache_key::CacheKey;
use crate::frame_store::decode_cache::DecodeCache;
use crate::frame_store::error::FrameStoreError;
use crate::frame_store::frame_peek::FramePeek;
use crate::frame_store::frame_quality::FrameQuality;
use crate::frame_store::frame_spill::FrameSpill;
use crate::frame_store::frame_stats::FrameStats;
use crate::frame_store::run_scratch::RunScratch;
use crate::ingest::frame_admission::FrameAdmission;
use crate::ingest::frame_step::FrameStep;
use crate::ingest::ingest_config::IngestConfig;
use crate::ingest::ingest_run::IngestRun;
use crate::io::image::error::ImageError;
use crate::io::image::image_metadata::ImageMetadata;
use crate::io::image::load_context::LoadContext;
use crate::memory::memory_plan::{MemoryPlan, RunShape};

use crate::frame_store::stackable_image::StackableImage;

use crate::frame_store::stored_frame::StoredFrame;
use crate::progress::progress_callback::ProgressCallback;
use crate::progress::stacking_progress::StackingStage;
use crate::progress::stage_counter::StageCounter;

use crate::combine::cache::core::{CacheCore, CacheTier};
use crate::combine::cache::frame_check::FrameCheck;

#[derive(Debug)]
struct LoadedTier {
    frames: Vec<StoredFrame>,
    spilled: bool,
    metadata: ImageMetadata,
}

/// [`load_tiered`] output: the loaded frames plus the assembled [`CacheCore`].
#[derive(Debug)]
pub(super) struct LoadedCache {
    pub(super) frames: Vec<StoredFrame>,
    pub(super) core: CacheCore,
}

/// Load every frame into the tier its set fits, each through `step` when given.
///
/// A prepared frame is not what its source decodes to, so on the disk tier it is spilled for this
/// run alone, never committed to the kept cache nor read back from it: a later run with another
/// step, or none, would otherwise reuse it.
pub(super) fn load_tiered<I: StackableImage, P: AsRef<Path> + Sync>(
    paths: &[P],
    config: &StackConfig,
    run: IngestRun,
    step: Option<&dyn FrameStep<I>>,
    progress: ProgressCallback,
) -> Result<LoadedCache, StackError> {
    debug_assert!(!paths.is_empty(), "`combine_cached` refuses an empty set");
    let first_path = paths[0].as_ref();
    let IngestRun { memory, context } = run;

    // Dimensions drive the in-memory-vs-disk tier decision. Peek the header without a decode when
    // the format allows it (RAW), so the in-memory path can decode every frame in parallel rather
    // than decoding frame 0 serially first; otherwise decode frame 0 and reuse it below.
    let (peek, early) = if let Some(peek) = I::peek(first_path, &context) {
        (peek, None)
    } else {
        let source = CachedSource::of(first_path);
        let mut image = load_image::<I>(first_path, &context)?;
        if let Some(step) = step {
            step.apply(0, &mut image)?;
        }
        (
            FramePeek::of_decoded(&image),
            Some(EarlyDecode { image, source }),
        )
    };
    let dimensions = peek.dimensions;
    // Frames of a set share a source, so frame 0 stands for all of them — including whether they
    // carry the two quality planes a masked frame does, which `FramePeek::resident_bytes` charges
    // for.
    let plan = MemoryPlan::plan(
        RunShape::decoded_stack(
            paths.len(),
            peek.resident_bytes(),
            dimensions.frame_bytes(),
            config.quality.resident_bytes(dimensions),
        ),
        rayon::current_num_threads(),
        memory.planning(),
    );

    tracing::info!(
        frame_count = paths.len(),
        sample_count = dimensions.sample_count(),
        planning_mb = memory.planning() / (1024 * 1024),
        use_in_memory = plan.fits_in_ram,
        "Image cache storage decision"
    );

    let admission = FrameAdmission::new(dimensions, &context.cancel);
    let load = TierLoad {
        paths,
        progress: &progress,
        admission: &admission,
        concurrency: plan.decode_concurrency,
        context: &context,
        step,
    };
    let LoadedTier {
        frames,
        spilled,
        metadata,
    } = if plan.fits_in_ram {
        load.in_memory(early.map(|early| early.image))?
    } else {
        load.to_disk(&config.ingest, early)?
    };

    Ok(LoadedCache {
        frames,
        core: CacheCore {
            tier: CacheTier::of(spilled, memory),
            dimensions,
            metadata,
            progress,
            cancel: context.cancel,
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

/// Frame 0 decoded ahead of the tier decision, because its format has no header to peek.
#[derive(Debug)]
struct EarlyDecode<I> {
    image: I,
    /// The source as it was before the decode, which only the disk tier reads — for its
    /// source-change check — so only the disk tier fails on it.
    source: Result<CachedSource, FrameStoreError>,
}

/// A decoded frame and its source as it was before the decode.
#[derive(Debug)]
struct Decoded<I> {
    image: I,
    source: CachedSource,
}

/// A source file as a kept cache names it: its canonical path, and its identity when it was read.
#[derive(Debug)]
struct CachedSource {
    canonical: PathBuf,
    identity: FileIdentity,
}

impl CachedSource {
    fn of(path: &Path) -> Result<Self, FrameStoreError> {
        let metadata_error = |source| FrameStoreError::ReadMetadata {
            path: path.to_path_buf(),
            source,
        };
        let canonical = fs::canonicalize(path).map_err(metadata_error)?;
        let identity = FileIdentity::of(&canonical).map_err(metadata_error)?;
        Ok(Self {
            canonical,
            identity,
        })
    }
}

/// A decoded frame that passed the per-frame checks, with what was measured on it.
#[derive(Debug)]
struct CheckedImage<I> {
    image: I,
    stats: FrameStats,
    quality: FrameQuality<Buffer2<f32>>,
}

impl<I: StackableImage> CheckedImage<I> {
    fn admit(image: I, index: usize, admission: &FrameAdmission<'_>) -> Result<Self, StackError> {
        Ok(Self {
            stats: admission.admit(index, &image)?,
            quality: FrameQuality::for_unwarped(&image),
            image,
        })
    }

    fn into_memory_frame(self) -> StoredFrame {
        StoredFrame::from_memory(self.image, self.quality, self.stats)
    }
}

/// A frame loaded into memory, with the stack metadata when it is frame 0.
#[derive(Debug)]
struct LoadedMemoryFrame {
    frame: StoredFrame,
    metadata: Option<ImageMetadata>,
}

/// One frame set's load into its tier: what both tiers read.
#[derive(Debug)]
struct TierLoad<'a, I, P> {
    paths: &'a [P],
    progress: &'a ProgressCallback,
    admission: &'a FrameAdmission<'a>,
    /// How many frames decode at once.
    concurrency: usize,
    context: &'a LoadContext,
    step: Option<&'a dyn FrameStep<I>>,
}

impl<I: StackableImage, P: AsRef<Path> + Sync> TierLoad<'_, I, P> {
    /// Load all images into memory and compute per-frame channel statistics.
    fn in_memory(&self, first: Option<I>) -> Result<LoadedTier, StackError> {
        let Self {
            paths,
            progress,
            admission,
            concurrency,
            context,
            step,
        } = *self;
        let cancel = &context.cancel;

        // When the header couldn't be peeked the caller pre-loaded frame 0, so the batch starts at
        // frame 1 and reuses it; otherwise every frame (frame 0 included) decodes in parallel. Frame 0
        // supplies the stack metadata either way.
        let loaded_count = StageCounter::new(progress, StackingStage::Loading, paths.len());
        let mut metadata = None;
        let mut frames = Vec::with_capacity(paths.len());
        if let Some(first_image) = first {
            metadata = Some(first_image.metadata().clone());
            frames.push(CheckedImage::admit(first_image, 0, admission)?.into_memory_frame());
            loaded_count.complete_one();
        }
        let start = frames.len();
        let loaded =
            concurrency::try_par_map_limited(&paths[start..], concurrency, |offset, path| {
                let index = offset + start;
                // Cancelled: stop decoding further frames (the slow phase).
                if cancel.is_cancelled() {
                    return Err(StackError::Cancelled);
                }
                let mut image = load_image::<I>(path.as_ref(), context)?;
                if let Some(step) = step {
                    step.apply(index, &mut image)?;
                }
                let loaded = LoadedMemoryFrame {
                    metadata: (index == 0).then(|| image.metadata().clone()),
                    frame: CheckedImage::admit(image, index, admission)?.into_memory_frame(),
                };
                loaded_count.complete_one();
                Ok(loaded)
            })?;
        for loaded in loaded {
            metadata = metadata.or(loaded.metadata);
            frames.push(loaded.frame);
        }

        tracing::info!("Loaded {} frames into memory", frames.len());
        Ok(LoadedTier {
            frames,
            spilled: false,
            metadata: metadata.expect("frame 0 provides metadata"),
        })
    }

    /// Load images to disk cache with memory-mapped access, each channel in a file of its own,
    /// decoding in parallel.
    fn to_disk(
        &self,
        config: &IngestConfig,
        early: Option<EarlyDecode<I>>,
    ) -> Result<LoadedTier, StackError> {
        let Self {
            paths,
            progress,
            admission,
            concurrency,
            context,
            step,
        } = *self;
        let scratch = RunScratch::create(&config.cache_dir)?;
        let kept = config
            .keep_cache
            .then(|| DecodeCache::open(&config.cache_dir))
            .transpose()?;
        let frame_cache = FrameDiskCache {
            scratch: &scratch,
            kept: kept.as_ref(),
            admission,
            context,
            step,
        };

        // Frame 0 first and alone: it carries the stack metadata, and the facts every later frame is
        // checked against as it decodes. It is always decoded, since only a decode yields the metadata;
        // its cache serves a later run in which it is not frame 0.
        let first_path = paths[0].as_ref();
        let first = if let Some(early) = early {
            Decoded {
                source: early.source?,
                image: early.image,
            }
        } else {
            let source = CachedSource::of(first_path)?;
            let mut image = load_image::<I>(first_path, context)?;
            if let Some(step) = step {
                step.apply(0, &mut image)?;
            }
            Decoded { image, source }
        };
        let metadata = first.image.metadata().clone();
        let first = frame_cache.frame(first_path, 0, Some(first))?;
        let cached_count = StageCounter::new(progress, StackingStage::Loading, paths.len());
        cached_count.complete_one();

        let remaining =
            concurrency::try_par_map_limited(&paths[1..], concurrency, |offset, path| {
                // Cancelled: stop decoding further frames (the slow phase).
                if context.cancel.is_cancelled() {
                    return Err(StackError::Cancelled);
                }
                let frame = frame_cache.frame(path.as_ref(), offset + 1, None)?;
                cached_count.complete_one();
                Ok(frame)
            })?;

        let mut frames = Vec::with_capacity(paths.len());
        frames.push(first);
        frames.extend(remaining);

        tracing::info!(
            "Spilled {} frames ({} channels each) to disk under {:?}",
            frames.len(),
            admission.dimensions().channels(),
            config.cache_dir
        );

        Ok(LoadedTier {
            frames,
            spilled: true,
            metadata,
        })
    }
}

/// The disk tier's frame store: the run's scratch, the decode cache when `keep_cache` asked for
/// one, and how a frame gets to either.
#[derive(Debug)]
struct FrameDiskCache<'a, I: StackableImage> {
    scratch: &'a RunScratch,
    kept: Option<&'a DecodeCache>,
    admission: &'a FrameAdmission<'a>,
    context: &'a LoadContext,
    step: Option<&'a dyn FrameStep<I>>,
}

impl<I: StackableImage> FrameDiskCache<'_, I> {
    /// Frame `index` on disk: with a decode cache, the planes it holds committed for this source
    /// and decoder, or a decode written there and committed; without one, a decode in the run's
    /// scratch.
    ///
    /// `decoded` is a frame already decoded, which is written rather than looked up: the decode a
    /// lookup would save is already spent. The source's identity is read before the decode and
    /// again after it, and a frame whose source changed in between is refused rather than cached
    /// under the identity it had before.
    ///
    /// A frame through `step` is not what its source decodes to, so it goes to the run's scratch
    /// whatever the cache.
    fn frame(
        &self,
        path: &Path,
        index: usize,
        decoded: Option<Decoded<I>>,
    ) -> Result<StoredFrame, StackError> {
        let dimensions = self.admission.dimensions();
        if let Some(step) = self.step {
            let image = if let Some(Decoded { image, .. }) = decoded {
                image
            } else {
                let mut image = load_image::<I>(path, self.context)?;
                step.apply(index, &mut image)?;
                image
            };
            let checked = CheckedImage::admit(image, index, self.admission)?;
            return StoredFrame::spill(
                self.scratch,
                &checked.image,
                &checked.quality,
                checked.stats,
            )
            .map_err(StackError::from);
        }
        let Some(kept) = self.kept else {
            let image = match decoded {
                Some(Decoded { image, .. }) => image,
                None => load_image::<I>(path, self.context)?,
            };
            let checked = CheckedImage::admit(image, index, self.admission)?;
            return StoredFrame::spill(
                self.scratch,
                &checked.image,
                &checked.quality,
                checked.stats,
            )
            .map_err(StackError::from);
        };
        let (source, decoded_image) = match decoded {
            Some(Decoded { image, source }) => (source, Some(image)),
            None => (CachedSource::of(path)?, None),
        };
        let spill = FrameSpill::cached(kept.path(), &source.canonical, I::DECODER);
        let key = CacheKey::new(source.identity, I::DECODER, self.context);

        if decoded_image.is_none()
            && let Some(frame) = StoredFrame::reuse(&spill, key, dimensions)?
        {
            tracing::debug!(source = %path.display(), "Reusing existing cache files");
            self.admission
                .check_facts(index, &frame.source_stats.facts)?;
            let check = FrameCheck {
                index,
                cancel: &self.context.cancel,
            };
            check.stored_samples(&frame.channels, dimensions.pixel_count())?;
            // Mapped from disk, so held to the pairing the combine divides by like any other planes.
            check.stored_quality(&frame, dimensions)?;
            return Ok(frame);
        }

        let image = match decoded_image {
            Some(image) => image,
            None => load_image::<I>(path, self.context)?,
        };
        let checked = CheckedImage::admit(image, index, self.admission)?;
        if CachedSource::of(path)?.identity != source.identity {
            return Err(FrameStoreError::SourceChanged {
                path: path.to_path_buf(),
            }
            .into());
        }
        StoredFrame::cache(&spill, key, &checked.image, &checked.quality, checked.stats)
            .map_err(StackError::from)
    }
}

#[cfg(test)]
mod tests;
