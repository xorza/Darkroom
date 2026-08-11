//! Tier selection, frame loading, and persistent cache sidecars.

use std::sync::OnceLock;

use std::io::Error as IoError;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use arrayvec::ArrayVec;
use common::CancelToken;
use common::SerdeFormat;
use common::file_utils;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use crate::concurrency;
use crate::io::image::cfa::CfaImage;
use crate::io::image::error::ImageError;
use crate::io::image::image_dimensions::ImageDimensions;
use crate::io::image::image_metadata::ImageMetadata;
use crate::io::image::linear::LinearImage;
use crate::io::image::load_context::LoadContext;
use crate::memory;
use crate::stacking::combine::cache_config::CacheConfig;
use crate::stacking::combine::config::Normalization;
use crate::stacking::combine::error::Error;
use crate::stacking::combine::normalization::compute_frame_norms;
use crate::stacking::frame_store::error::FrameStoreError;
use crate::stacking::frame_store::frame_stats::FrameStats;
use crate::stacking::frame_store::spill::FrameSpill;
use crate::stacking::frame_store::spill::SpillDirectory;
use crate::stacking::frame_store::stored_plane::StoredPlane;
use crate::stacking::frame_store::warp_quality::WarpQuality;
use crate::stacking::frame_store::{StackableImage, StoredFrame};
use crate::stacking::progress::{ProgressCallback, StackingStage};

use crate::stacking::combine::cache::validation::{
    validate_image_samples, validate_stored_samples,
};
use crate::stacking::combine::cache::{CacheCore, FrameCache};

#[derive(Debug)]
struct LoadedTier {
    frames: Vec<StoredFrame>,
    spill_directory: Option<SpillDirectory>,
    metadata: ImageMetadata,
}

/// [`load_tiered`] output: the loaded frames plus the assembled [`CacheCore`].
#[derive(Debug)]
struct LoadedCache {
    frames: Vec<StoredFrame>,
    core: CacheCore,
}

fn load_tiered<I: StackableImage, P: AsRef<Path> + Sync>(
    paths: &[P],
    config: &CacheConfig,
    progress: ProgressCallback,
    cancel: CancelToken,
) -> Result<LoadedCache, Error> {
    if paths.is_empty() {
        return Err(Error::NoFrames);
    }

    progress.report(0, paths.len(), StackingStage::Loading);

    let first_path = paths[0].as_ref();
    // One reading, shared by the tier decision below and the decode ceiling the context carries;
    // `LoadContext::default()` would sample the machine a second time and could disagree.
    // One system reading: it is the decode ceiling the context carries, and the fallback for the
    // tier figure when the config has no planning override. The override deliberately does not
    // reach the context — it says how to tier, not how much one file may allocate.
    let system_available = memory::available_memory();
    let available_memory = config.available_memory_or(system_available);
    let context = LoadContext::new(cancel.clone(), memory::memory_budget(system_available));

    // Dimensions drive the in-memory-vs-disk tier decision. Peek the header without a decode when
    // the format allows it (RAW), so the in-memory path can decode every frame in parallel rather
    // than decoding frame 0 serially first; otherwise decode frame 0 and reuse it below.
    let (dimensions, first_image) = match I::peek_dimensions(first_path, &context) {
        Some(dims) => (dims, None),
        None => {
            let img = load_image::<I>(first_path, &context)?;
            (img.dimensions(), Some(img))
        }
    };
    let use_in_memory = memory::fits_in_memory(
        memory::frame_bytes(dimensions),
        paths.len(),
        available_memory,
    );

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
            first_image,
            available_memory,
            &context,
        )?
    } else {
        // Disk tier (large stacks): the serial-first-frame path. If the header was peeked we
        // haven't decoded frame 0 yet, so decode it now — rare, since calibration fits in RAM.
        let first = match first_image {
            Some(img) => img,
            None => load_image::<I>(first_path, &context)?,
        };
        load_to_disk::<I, P>(
            paths,
            config,
            &progress,
            dimensions,
            first,
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
        Err(ImageError::Cancelled { .. }) => Err(Error::Cancelled),
        Err(source) => Err(Error::ImageLoad {
            path: path.to_path_buf(),
            source: IoError::other(source),
        }),
    }
}

impl FrameCache {
    /// Build a cache from CFA calibration frame files (tiered in-memory/disk per available RAM).
    pub(crate) fn from_cfa_paths<P: AsRef<Path> + Sync>(
        paths: &[P],
        config: &CacheConfig,
        normalization: Normalization,
        progress: ProgressCallback,
        cancel: CancelToken,
    ) -> Result<Self, Error> {
        Self::from_tiered_paths(
            load_tiered::<CfaImage, P>(paths, config, progress, cancel)?,
            normalization,
        )
    }

    /// Build a cache from light-frame image files (tiered per available RAM). Files on disk carry
    /// no warp quality planes, so every pixel has full support and unit confidence.
    pub(crate) fn from_paths<P: AsRef<Path> + Sync>(
        paths: &[P],
        config: &CacheConfig,
        normalization: Normalization,
        progress: ProgressCallback,
        cancel: CancelToken,
    ) -> Result<Self, Error> {
        Self::from_tiered_paths(
            load_tiered::<LinearImage, P>(paths, config, progress, cancel)?,
            normalization,
        )
    }

    fn from_tiered_paths(loaded: LoadedCache, normalization: Normalization) -> Result<Self, Error> {
        let LoadedCache { frames, core } = loaded;
        let frame_norms =
            compute_frame_norms(&frames, core.dimensions, normalization, &core.cancel)?;
        Ok(Self {
            frames,
            frame_norms,
            normalization,
            core,
        })
    }
}

#[derive(Debug)]
struct LoadedMemoryFrame {
    frame: StoredFrame,
    metadata: Option<ImageMetadata>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct SourceIdentity {
    canonical_path: Vec<u8>,
    byte_len: u64,
    modified_nanos: i128,
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
    // supplies the stack metadata either way.
    let start = if first.is_some() { 1 } else { 0 };
    let loaded = concurrency::try_par_map_limited(&paths[start..], concurrency, |offset, path| {
        let idx = offset + start;
        // Cancelled: stop decoding further frames (the slow phase).
        if cancel.is_cancelled() {
            return Err(Error::Cancelled);
        }
        let image = load_image::<I>(path.as_ref(), context)?;
        if image.dimensions() != dimensions {
            return Err(Error::DimensionMismatch {
                index: idx,
                expected: dimensions,
                actual: image.dimensions(),
            });
        }
        validate_image_samples(&image, idx, cancel)?;
        let metadata = (idx == 0).then(|| image.metadata().clone());
        let stats = FrameStats::measure(&image);
        Ok(LoadedMemoryFrame {
            frame: StoredFrame::from_memory(image, WarpQuality::None, stats),
            metadata,
        })
    })?;

    let mut frames = Vec::with_capacity(paths.len());
    let mut metadata = None;
    if let Some(first_image) = first {
        validate_image_samples(&first_image, 0, cancel)?;
        metadata = Some(first_image.metadata().clone());
        let stats = FrameStats::measure(&first_image);
        frames.push(StoredFrame::from_memory(
            first_image,
            WarpQuality::None,
            stats,
        ));
    }
    for loaded_frame in loaded {
        if loaded_frame.metadata.is_some() {
            metadata = loaded_frame.metadata;
        }
        frames.push(loaded_frame.frame);
    }

    progress.report(paths.len(), paths.len(), StackingStage::Loading);

    tracing::info!("Loaded {} frames into memory", frames.len());
    Ok(LoadedTier {
        frames,
        spill_directory: None,
        metadata: metadata.expect("frame 0 provides metadata"),
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
    first_image: I,
    available_memory: u64,
    context: &LoadContext,
) -> Result<LoadedTier, Error> {
    let cancel = &context.cancel;
    let spill_directory = SpillDirectory::create(config.cache_dir.clone(), config.keep_cache)?;
    let cache_dir = &spill_directory.path;

    // Cache first image and compute stats. Frame 0 carries the stack metadata.
    validate_image_samples(&first_image, 0, cancel)?;
    let metadata = first_image.metadata().clone();
    let first_stats = FrameStats::measure(&first_image);
    let first_path = paths[0].as_ref();
    let base_filename = FrameSpill::cache_name(first_path);
    let first_cached = StoredFrame::spill(
        cache_dir,
        &base_filename,
        &first_image,
        WarpQuality::None,
        first_stats,
    )
    .map_err(Error::from)?;
    progress.report(1, paths.len(), StackingStage::Loading);

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
        if cancel.is_cancelled() {
            return Err(Error::Cancelled);
        }
        let path = path.as_ref();
        let base_filename = FrameSpill::cache_name(path);
        load_and_cache_frame::<I>(
            cache_dir,
            &base_filename,
            path,
            dimensions,
            offset + 1,
            context,
        )
    })?;

    let mut frames = Vec::with_capacity(paths.len());
    frames.push(first_cached);
    frames.extend(remaining);

    progress.report(paths.len(), paths.len(), StackingStage::Loading);

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

fn source_identity(path: &Path) -> Result<SourceIdentity, FrameStoreError> {
    let canonical =
        std::fs::canonicalize(path).map_err(|source| FrameStoreError::ReadMetadata {
            path: path.to_path_buf(),
            source,
        })?;
    let metadata =
        std::fs::metadata(&canonical).map_err(|source| FrameStoreError::ReadMetadata {
            path: path.to_path_buf(),
            source,
        })?;
    let modified = metadata
        .modified()
        .map_err(|source| FrameStoreError::ReadMetadata {
            path: path.to_path_buf(),
            source,
        })?;
    let modified_nanos = match modified.duration_since(UNIX_EPOCH) {
        Ok(duration) => duration.as_nanos() as i128,
        Err(error) => -(error.duration().as_nanos() as i128),
    };
    Ok(SourceIdentity {
        canonical_path: canonical.as_os_str().as_encoded_bytes().to_vec(),
        byte_len: metadata.len(),
        modified_nanos,
    })
}

/// Layout tag carried by every sidecar.
///
/// Bitcode is not self-describing, so a file written by a build whose sidecar structs differed
/// would otherwise decode into plausible nonsense instead of being rejected. Bump this whenever
/// [`SourceIdentity`] or [`FrameStats`] changes shape; a cache that fails the check is simply
/// re-decoded.
const SIDECAR_FORMAT: u32 = 1;

/// A sidecar payload behind its layout tag.
#[derive(Debug, Serialize, Deserialize)]
struct Sidecar<T> {
    format: u32,
    value: T,
}

fn write_sidecar_value<T: Serialize>(path: PathBuf, value: &T) -> Result<(), FrameStoreError> {
    let sidecar = Sidecar {
        format: SIDECAR_FORMAT,
        value,
    };
    // Sidecars are plain scalars and a byte vector; a failure here would be a broken derive, not
    // anything the filesystem or the caller can cause.
    let bytes = common::serialize(&sidecar, SerdeFormat::Bitcode)
        .expect("a sidecar of plain scalars always serializes");
    write_sidecar(path, &bytes)
}

/// Read a sidecar back, or `None` if it is absent, unreadable, or not this layout.
fn read_sidecar_value<T: DeserializeOwned>(path: &Path) -> Option<T> {
    let bytes = std::fs::read(path).ok()?;
    let sidecar: Sidecar<T> = common::deserialize(&bytes, SerdeFormat::Bitcode).ok()?;
    (sidecar.format == SIDECAR_FORMAT).then_some(sidecar.value)
}

fn meta_path(cache_dir: &Path, base_filename: &str) -> PathBuf {
    cache_dir.join(format!("{}.meta", base_filename.trim_end_matches(".bin")))
}

fn write_source_meta(
    cache_dir: &Path,
    base_filename: &str,
    identity: &SourceIdentity,
) -> Result<(), FrameStoreError> {
    write_sidecar_value(meta_path(cache_dir, base_filename), identity)
}

fn validate_source_meta(cache_dir: &Path, base_filename: &str, identity: &SourceIdentity) -> bool {
    read_sidecar_value::<SourceIdentity>(&meta_path(cache_dir, base_filename))
        .is_some_and(|stored| stored == *identity)
}

/// Load an image and cache it, or reuse existing cache files if valid.
fn load_and_cache_frame<I: StackableImage>(
    cache_dir: &Path,
    base_filename: &str,
    source_path: &Path,
    dimensions: ImageDimensions,
    frame_index: usize,
    context: &LoadContext,
) -> Result<StoredFrame, Error> {
    let cancel = &context.cancel;
    let channels = dimensions.channels();
    let identity_before = source_identity(source_path)?;

    // Check if all channel files exist, have correct size, and source hasn't changed
    let spill = FrameSpill::new(cache_dir, base_filename);
    let meta_valid = validate_source_meta(cache_dir, base_filename, &identity_before);
    let cached_stats = read_frame_stats(cache_dir, base_filename);
    let can_reuse = meta_valid && cached_stats.is_some() && spill.channels_reusable(dimensions);

    if can_reuse {
        // Reuse existing cache files - just mmap them
        let mut planes = ArrayVec::new();
        for c in 0..channels {
            planes.push(StoredPlane::map(spill.channel_path(c))?);
        }
        tracing::debug!(
            source = %source_path.display(),
            "Reusing existing cache files"
        );
        let frame = StoredFrame {
            channels: planes,
            quality: WarpQuality::None,
            source_stats: cached_stats.expect("valid cache has readable frame statistics"),
        };
        validate_stored_samples(
            &frame.channels,
            dimensions.pixel_count(),
            frame_index,
            cancel,
        )?;
        Ok(frame)
    } else {
        // Load image and write to cache
        let image = load_image::<I>(source_path, context)?;

        if image.dimensions() != dimensions {
            return Err(Error::DimensionMismatch {
                index: frame_index,
                expected: dimensions,
                actual: image.dimensions(),
            });
        }
        validate_image_samples(&image, frame_index, cancel)?;
        let identity_after = source_identity(source_path)?;
        if identity_after != identity_before {
            return Err(FrameStoreError::SourceChanged {
                path: source_path.to_path_buf(),
            }
            .into());
        }

        let stats = FrameStats::measure(&image);
        let stored = StoredFrame::spill(
            cache_dir,
            base_filename,
            &image,
            WarpQuality::None,
            stats.clone(),
        )
        .map_err(Error::from)?;

        // The identity sidecar is the commit record for the planes and stats.
        write_frame_stats(cache_dir, base_filename, &stats)?;
        write_source_meta(cache_dir, base_filename, &identity_after)?;

        Ok(stored)
    }
}

/// Path for the sidecar stats file.
fn stats_path(cache_dir: &Path, base_filename: &str) -> PathBuf {
    cache_dir.join(format!("{}.stats", base_filename.trim_end_matches(".bin")))
}

/// Write frame stats to a sidecar file.
fn write_frame_stats(
    cache_dir: &Path,
    base_filename: &str,
    stats: &FrameStats,
) -> Result<(), FrameStoreError> {
    write_sidecar_value(stats_path(cache_dir, base_filename), stats)
}

fn write_sidecar(path: PathBuf, bytes: &[u8]) -> Result<(), FrameStoreError> {
    file_utils::publish_bytes(&path, bytes, file_utils::PublicationMode::Cache)
        .map_err(|source| FrameStoreError::WriteFile { path, source })
}

/// Read frame stats from a sidecar file.
fn read_frame_stats(cache_dir: &Path, base_filename: &str) -> Option<FrameStats> {
    let stats: FrameStats = read_sidecar_value(&stats_path(cache_dir, base_filename))?;
    // A file can decode cleanly and still hold a sigma that would poison every weight derived
    // from it, so the value is checked rather than just the layout. Rejecting means re-decoding
    // the frame, not failing the run.
    if stats
        .quantization_sigma
        .is_some_and(|sigma| !sigma.is_finite() || sigma <= 0.0)
    {
        return None;
    }
    Some(stats)
}

#[cfg(test)]
mod tests;
