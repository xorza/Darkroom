//! Tier selection, frame loading, and persistent cache sidecars.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use common::{CancelToken, FileIdentity};
use imaginarium::Buffer2;

use crate::concurrency;
use crate::error::FrameDimensionMismatch;
use crate::io::image::error::ImageError;
use crate::io::image::image_dimensions::ImageDimensions;
use crate::io::image::image_metadata::ImageMetadata;
use crate::io::image::load_context::LoadContext;
use crate::memory;
use crate::stacking::combine::cache_config::CacheConfig;
use crate::stacking::combine::error::Error;
use crate::stacking::frame_store::cache_key::CacheKey;
use crate::stacking::frame_store::error::FrameStoreError;
use crate::stacking::frame_store::frame_quality::FrameQuality;
use crate::stacking::frame_store::frame_stats::FrameStats;
use crate::stacking::frame_store::spill::FrameSpill;
use crate::stacking::frame_store::spill_directory::SpillDirectory;
use crate::stacking::frame_store::{FramePeek, StackableImage, StoredFrame};
use crate::stacking::progress::stage_counter::StageCounter;
use crate::stacking::progress::{ProgressCallback, StackingStage};

use crate::stacking::combine::cache::CacheCore;
use crate::stacking::combine::cache::set_facts::SetFacts;
use crate::stacking::combine::cache::validation::{
    validate_image_samples, validate_stored_quality, validate_stored_samples,
};
use crate::stacking::frame_store::frame_facts::FrameFacts;

#[derive(Debug)]
struct LoadedTier {
    frames: Vec<StoredFrame>,
    spill_directory: Option<SpillDirectory>,
    metadata: ImageMetadata,
}

/// [`load_tiered`] output: the loaded frames plus the assembled [`CacheCore`].
#[derive(Debug)]
pub(super) struct LoadedCache {
    pub(super) frames: Vec<StoredFrame>,
    pub(super) core: CacheCore,
}

pub(super) fn load_tiered<I: StackableImage, P: AsRef<Path> + Sync>(
    paths: &[P],
    config: &CacheConfig,
    progress: ProgressCallback,
    cancel: CancelToken,
) -> Result<LoadedCache, Error> {
    if paths.is_empty() {
        return Err(Error::NoFrames);
    }

    let first_path = paths[0].as_ref();
    // One system reading for the whole load: the config is resolved against it here so the tier
    // decision below, the cache the frames end up in, and the decode ceiling the context carries all
    // size against the same figure. Sampling again downstream — as an unresolved config or
    // `LoadContext::default()` would — could answer a different number. The planning override
    // deliberately does not reach the context: it says how to tier, not how much one file may
    // allocate.
    let system_available = memory::available_memory();
    let config = &config.resolved_with(system_available);
    let available_memory = config.planning_memory();
    let context = LoadContext::new(cancel.clone(), memory::memory_budget(system_available));

    // Dimensions drive the in-memory-vs-disk tier decision. Peek the header without a decode when
    // the format allows it (RAW), so the in-memory path can decode every frame in parallel rather
    // than decoding frame 0 serially first; otherwise decode frame 0 and reuse it below.
    let (peek, early) = if let Some(peek) = I::peek(first_path, &context) {
        (peek, None)
    } else {
        let source = CachedSource::of(first_path);
        let image = load_image::<I>(first_path, &context)?;
        (
            FramePeek::of_decoded(&image),
            Some(EarlyDecode { image, source }),
        )
    };
    let dimensions = peek.dimensions;
    // Frames of a set share a source, so frame 0 stands for all of them — including whether they
    // carry the two quality planes a masked frame does, which `FramePeek::resident_bytes` charges
    // for.
    let use_in_memory =
        memory::fits_in_memory(peek.resident_bytes(), paths.len(), available_memory);

    tracing::info!(
        frame_count = paths.len(),
        sample_count = dimensions.sample_count(),
        available_mb = available_memory / (1024 * 1024),
        use_in_memory,
        "Image cache storage decision"
    );

    let LoadedTier {
        frames,
        spill_directory,
        metadata,
    } = if use_in_memory {
        load_in_memory::<I, P>(
            paths,
            &progress,
            dimensions,
            early.map(|early| early.image),
            available_memory,
            &context,
        )?
    } else {
        load_to_disk::<I, P>(
            paths,
            config,
            &progress,
            dimensions,
            early,
            available_memory,
            &context,
        )?
    };

    Ok(LoadedCache {
        frames,
        core: CacheCore {
            spill_directory,
            dimensions,
            metadata,
            config: config.clone(),
            progress,
            cancel,
            chunk_memory: OnceLock::new(),
        },
    })
}

fn load_image<I: StackableImage>(path: &Path, context: &LoadContext) -> Result<I, Error> {
    match I::load(path, context) {
        Ok(image) => Ok(image),
        // Cancellation is the run stopping, not this file failing, so it leaves the load error
        // behind and reports as the stack's own.
        Err(ImageError::Cancelled { .. }) => Err(Error::Cancelled),
        Err(source) => Err(Error::ImageLoad(source)),
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

/// A decoded frame's per-frame checks, in the order `validate_frame` runs them — geometry, its
/// facts against `facts` when the set's are known, then its samples — and its statistics and
/// quality planes.
fn check_decoded<I: StackableImage>(
    image: I,
    index: usize,
    dimensions: ImageDimensions,
    facts: Option<&SetFacts>,
    cancel: &CancelToken,
) -> Result<CheckedImage<I>, Error> {
    FrameDimensionMismatch::check(index, dimensions, image.dimensions())?;
    if let Some(facts) = facts {
        facts.check(index, &FrameFacts::of(&image))?;
    }
    validate_image_samples(&image, index, cancel)?;
    Ok(CheckedImage {
        stats: FrameStats::measure(&image),
        quality: FrameQuality::for_unwarped(&image),
        image,
    })
}

#[derive(Debug)]
struct LoadedMemoryFrame {
    frame: StoredFrame,
    metadata: Option<ImageMetadata>,
}

/// Load all images into memory and compute per-frame channel statistics.
fn load_in_memory<I: StackableImage, P: AsRef<Path> + Sync>(
    paths: &[P],
    progress: &ProgressCallback,
    dimensions: ImageDimensions,
    first: Option<I>,
    available_memory: u64,
    context: &LoadContext,
) -> Result<LoadedTier, Error> {
    let cancel = &context.cancel;
    // Decode is CPU-bound, so fan out to the worker count, bounded by RAM headroom — every frame
    // stays resident in this tier, so only the budget left over feeds in-flight decode transients,
    // each charged its true ~2× footprint (`decode_transient_bytes`) so the load doesn't overshoot.
    let concurrency = memory::load_concurrency(
        memory::frame_bytes(dimensions),
        memory::decode_transient_bytes(dimensions),
        paths.len(),
        available_memory,
        rayon::current_num_threads(),
    );

    // When the header couldn't be peeked the caller pre-loaded frame 0, so the batch starts at
    // frame 1 and reuses it; otherwise every frame (frame 0 included) decodes in parallel. Frame 0
    // supplies the stack metadata either way, and its facts as soon as it has decoded, so a frame
    // that disagrees stops the load before the rest of the set decodes.
    let first_facts = OnceLock::new();
    let loaded_count = StageCounter::new(progress, StackingStage::Loading, paths.len());
    let mut first_frame = None;
    if let Some(first_image) = first {
        let frame = admit_decoded(first_image, 0, dimensions, &first_facts, cancel)?;
        loaded_count.complete_one();
        first_frame = Some(frame);
    }
    let start = usize::from(first_frame.is_some());
    let loaded = concurrency::try_par_map_limited(&paths[start..], concurrency, |offset, path| {
        let idx = offset + start;
        // Cancelled: stop decoding further frames (the slow phase).
        if cancel.is_cancelled() {
            return Err(Error::Cancelled);
        }
        let image = load_image::<I>(path.as_ref(), context)?;
        let frame = admit_decoded(image, idx, dimensions, &first_facts, cancel)?;
        loaded_count.complete_one();
        Ok(frame)
    })?;

    let mut frames = Vec::with_capacity(paths.len());
    let mut metadata = None;
    if let Some(first_frame) = first_frame {
        metadata = first_frame.metadata;
        frames.push(first_frame.frame);
    }
    for loaded_frame in loaded {
        if loaded_frame.metadata.is_some() {
            metadata = loaded_frame.metadata;
        }
        frames.push(loaded_frame.frame);
    }

    tracing::info!("Loaded {} frames into memory", frames.len());
    Ok(LoadedTier {
        frames,
        spill_directory: None,
        metadata: metadata.expect("frame 0 provides metadata"),
    })
}

/// [`check_decoded`] for a frame decoding beside the others: frame 0 publishes its facts for the
/// rest, and each later frame checks against them once they are known. The full in-order check of
/// the set runs once all are in.
fn admit_decoded<I: StackableImage>(
    image: I,
    index: usize,
    dimensions: ImageDimensions,
    first_facts: &OnceLock<SetFacts>,
    cancel: &CancelToken,
) -> Result<LoadedMemoryFrame, Error> {
    let metadata = (index == 0).then(|| image.metadata().clone());
    let checked = check_decoded(image, index, dimensions, first_facts.get(), cancel)?;
    if index == 0 {
        first_facts
            .set(SetFacts::of_first(&checked.stats.facts))
            .expect("frame 0 decodes once");
    }
    Ok(LoadedMemoryFrame {
        frame: StoredFrame::from_memory(checked.image, checked.quality, checked.stats),
        metadata,
    })
}

/// Load images to disk cache with memory-mapped access.
/// Each channel is stored in a separate file for efficient planar access.
/// Images are loaded and cached in parallel for better throughput.
fn load_to_disk<I: StackableImage, P: AsRef<Path> + Sync>(
    paths: &[P],
    config: &CacheConfig,
    progress: &ProgressCallback,
    dimensions: ImageDimensions,
    early: Option<EarlyDecode<I>>,
    available_memory: u64,
    context: &LoadContext,
) -> Result<LoadedTier, Error> {
    let spill_directory = SpillDirectory::create(&config.cache_dir, config.keep_cache)?;
    let cache_dir = spill_directory.path();

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
        Decoded {
            image: load_image::<I>(first_path, context)?,
            source,
        }
    };
    let metadata = first.image.metadata().clone();
    let first = cache_frame::<I>(
        cache_dir,
        first_path,
        0,
        dimensions,
        None,
        context,
        Some(first),
    )?;
    let first_facts = SetFacts::of_first(&first.source_stats.facts);
    let cached_count = StageCounter::new(progress, StackingStage::Loading, paths.len());
    cached_count.complete_one();

    // Decode is CPU-bound, so fan out to the worker count, bounded by RAM. The disk tier streams
    // each decoded frame to its own file and drops it, so nothing stays resident (`0`) — only the
    // in-flight decodes occupy memory, each its true ~2× transient. Each frame writes unique files,
    // so there's no contention.
    let concurrency = memory::load_concurrency(
        memory::frame_bytes(dimensions),
        memory::decode_transient_bytes(dimensions),
        0,
        available_memory,
        rayon::current_num_threads(),
    );
    let remaining = concurrency::try_par_map_limited(&paths[1..], concurrency, |offset, path| {
        // Cancelled: stop decoding further frames (the slow phase).
        if context.cancel.is_cancelled() {
            return Err(Error::Cancelled);
        }
        let frame = cache_frame::<I>(
            cache_dir,
            path.as_ref(),
            offset + 1,
            dimensions,
            Some(&first_facts),
            context,
            None,
        )?;
        cached_count.complete_one();
        Ok(frame)
    })?;

    let mut frames = Vec::with_capacity(paths.len());
    frames.push(first);
    frames.extend(remaining);

    tracing::info!(
        "Cached {} frames ({} channels each) to disk at {:?}",
        frames.len(),
        dimensions.channels(),
        cache_dir
    );

    Ok(LoadedTier {
        frames,
        spill_directory: Some(spill_directory),
        metadata,
    })
}

/// Frame `index` through the cache in `cache_dir`: the planes a committed cache holds for this
/// source and decoder, or a decode written there and committed.
///
/// `decoded` is a frame already decoded, which is written rather than looked up: the decode a
/// lookup would save is already spent. The source's identity is read before the decode and again
/// after it, and a frame whose source changed in between is refused rather than cached under the
/// identity it had before.
fn cache_frame<I: StackableImage>(
    cache_dir: &Path,
    path: &Path,
    index: usize,
    dimensions: ImageDimensions,
    facts: Option<&SetFacts>,
    context: &LoadContext,
    decoded: Option<Decoded<I>>,
) -> Result<StoredFrame, Error> {
    let cancel = &context.cancel;
    let (source, decoded_image) = match decoded {
        Some(Decoded { image, source }) => (source, Some(image)),
        None => (CachedSource::of(path)?, None),
    };
    let spill = FrameSpill::cached(cache_dir, &source.canonical, I::DECODER);
    let key = CacheKey::new(source.identity, I::DECODER);

    if decoded_image.is_none()
        && let Some(frame) = StoredFrame::reuse(&spill, key, dimensions)?
    {
        tracing::debug!(source = %path.display(), "Reusing existing cache files");
        if let Some(facts) = facts {
            facts.check(index, &frame.source_stats.facts)?;
        }
        validate_stored_samples(&frame.channels, dimensions.pixel_count(), index, cancel)?;
        // Mapped from disk, so held to the pairing the combine divides by like any other planes.
        validate_stored_quality(index, &frame, dimensions, cancel)?;
        return Ok(frame);
    }

    let image = match decoded_image {
        Some(image) => image,
        None => load_image::<I>(path, context)?,
    };
    let checked = check_decoded(image, index, dimensions, facts, cancel)?;
    if CachedSource::of(path)?.identity != source.identity {
        return Err(FrameStoreError::SourceChanged {
            path: path.to_path_buf(),
        }
        .into());
    }
    StoredFrame::cache(&spill, key, &checked.image, &checked.quality, checked.stats)
        .map_err(Error::from)
}

#[cfg(test)]
mod tests;
