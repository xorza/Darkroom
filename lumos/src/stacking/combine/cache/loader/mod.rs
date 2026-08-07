//! Tier selection, frame loading, and persistent cache sidecars.

use std::io::Error as IoError;
use std::mem::size_of;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use arrayvec::ArrayVec;
use common::CancelToken;
use common::file_utils;

use crate::concurrency;
use crate::io::image::LoadContext;
use crate::io::image::cfa::CfaImage;
use crate::io::image::error::ImageError;
use crate::io::image::linear::LinearImage;
use crate::io::image::{ImageDimensions, ImageMetadata};
use crate::math::statistics::ChannelStats;
use crate::memory::{decode_transient_bytes, fits_in_memory, frame_bytes, load_concurrency};
use crate::stacking::combine::cache_config::CacheConfig;
use crate::stacking::combine::config::Normalization;
use crate::stacking::combine::error::Error;
use crate::stacking::combine::normalization::compute_frame_norms;
use crate::stacking::frame_store::{
    FrameSpill, FrameStats, FrameStoreError, SpillDirectory, StackableImage, StoredFrame,
    StoredPlane,
};
use crate::stacking::progress::{ProgressCallback, StackingStage};

use crate::stacking::combine::cache::{
    CacheCore, FrameCache, validate_image_samples, validate_stored_samples,
};

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
    let available_memory = config.get_available_memory();
    let context = LoadContext {
        cancel: cancel.clone(),
        ..LoadContext::default()
    };

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
    let use_in_memory = fits_in_memory(frame_bytes(dimensions), paths.len(), available_memory);

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

#[derive(Debug, Clone, PartialEq, Eq)]
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
    let concurrency = load_concurrency(
        frame_bytes(dimensions),
        decode_transient_bytes(dimensions),
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
            frame: StoredFrame::from_memory(image, None, None, stats),
            metadata,
        })
    })?;

    let mut frames = Vec::with_capacity(paths.len());
    let mut metadata = None;
    if let Some(first_image) = first {
        validate_image_samples(&first_image, 0, cancel)?;
        metadata = Some(first_image.metadata().clone());
        let stats = FrameStats::measure(&first_image);
        frames.push(StoredFrame::from_memory(first_image, None, None, stats));
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
        None,
        None,
        first_stats,
    )
    .map_err(Error::from)?;
    progress.report(1, paths.len(), StackingStage::Loading);

    // Decode is CPU-bound, so fan out to the worker count, bounded by RAM. The disk tier streams
    // each decoded frame to its own file and drops it, so nothing stays resident (`0`) — only the
    // in-flight decodes occupy memory, each its true ~2× transient. Each frame writes unique files,
    // so there's no contention.
    let concurrency = load_concurrency(
        frame_bytes(dimensions),
        decode_transient_bytes(dimensions),
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

const SOURCE_META_MAGIC: [u8; 8] = *b"LUMSRC01";

fn encode_source_identity(identity: &SourceIdentity) -> Vec<u8> {
    let path_len =
        u32::try_from(identity.canonical_path.len()).expect("a filesystem path length fits in u32");
    let mut bytes = Vec::with_capacity(
        SOURCE_META_MAGIC.len()
            + size_of::<u32>()
            + identity.canonical_path.len()
            + size_of::<u64>()
            + size_of::<i128>(),
    );
    bytes.extend_from_slice(&SOURCE_META_MAGIC);
    bytes.extend_from_slice(&path_len.to_le_bytes());
    bytes.extend_from_slice(&identity.canonical_path);
    bytes.extend_from_slice(&identity.byte_len.to_le_bytes());
    bytes.extend_from_slice(&identity.modified_nanos.to_le_bytes());
    bytes
}

fn decode_source_identity(bytes: &[u8]) -> Option<SourceIdentity> {
    let path_len_offset = SOURCE_META_MAGIC.len();
    let path_offset = path_len_offset + size_of::<u32>();
    if bytes.get(..path_len_offset)? != SOURCE_META_MAGIC {
        return None;
    }
    let path_len = u32::from_le_bytes(
        bytes
            .get(path_len_offset..path_offset)?
            .try_into()
            .expect("source path length is four bytes"),
    ) as usize;
    let byte_len_offset = path_offset.checked_add(path_len)?;
    let modified_offset = byte_len_offset.checked_add(size_of::<u64>())?;
    let expected_len = modified_offset.checked_add(size_of::<i128>())?;
    if bytes.len() != expected_len {
        return None;
    }
    Some(SourceIdentity {
        canonical_path: bytes[path_offset..byte_len_offset].to_vec(),
        byte_len: u64::from_le_bytes(
            bytes[byte_len_offset..modified_offset]
                .try_into()
                .expect("source byte length is eight bytes"),
        ),
        modified_nanos: i128::from_le_bytes(
            bytes[modified_offset..]
                .try_into()
                .expect("source timestamp is sixteen bytes"),
        ),
    })
}

fn meta_path(cache_dir: &Path, base_filename: &str) -> PathBuf {
    cache_dir.join(format!("{}.meta", base_filename.trim_end_matches(".bin")))
}

fn write_source_meta(
    cache_dir: &Path,
    base_filename: &str,
    identity: &SourceIdentity,
) -> Result<(), FrameStoreError> {
    let path = meta_path(cache_dir, base_filename);
    write_sidecar(path, &encode_source_identity(identity))
}

fn validate_source_meta(cache_dir: &Path, base_filename: &str, identity: &SourceIdentity) -> bool {
    let path = meta_path(cache_dir, base_filename);
    let Ok(bytes) = std::fs::read(&path) else {
        return false;
    };
    decode_source_identity(&bytes).is_some_and(|stored| stored == *identity)
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
            coverage: None,
            confidence: None,
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
        let stored =
            StoredFrame::spill(cache_dir, base_filename, &image, None, None, stats.clone())
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
/// Format: [n_channels: u8] [has_quantization: u8] [quantization_sigma: f32]
/// [median_0: f32] [mad_0: f32] [median_1: f32] ...
fn write_frame_stats(
    cache_dir: &Path,
    base_filename: &str,
    stats: &FrameStats,
) -> Result<(), FrameStoreError> {
    let path = stats_path(cache_dir, base_filename);
    let n = stats.channels.len();
    let mut buf = Vec::with_capacity(6 + n * 8);
    buf.push(n as u8);
    buf.push(u8::from(stats.quantization_sigma.is_some()));
    buf.extend_from_slice(&stats.quantization_sigma.unwrap_or_default().to_le_bytes());
    for ch in &stats.channels {
        buf.extend_from_slice(&ch.median.to_le_bytes());
        buf.extend_from_slice(&ch.mad.to_le_bytes());
    }
    write_sidecar(path, &buf)
}

fn write_sidecar(path: PathBuf, bytes: &[u8]) -> Result<(), FrameStoreError> {
    file_utils::publish_bytes(&path, bytes, file_utils::PublicationMode::Cache)
        .map_err(|source| FrameStoreError::WriteFile { path, source })
}

/// Read frame stats from a sidecar file.
fn read_frame_stats(cache_dir: &Path, base_filename: &str) -> Option<FrameStats> {
    let path = stats_path(cache_dir, base_filename);
    let bytes = std::fs::read(&path).ok()?;
    if bytes.len() < 6 {
        return None;
    }
    let n = bytes[0] as usize;
    if bytes.len() != 6 + n * 8 {
        return None;
    }
    let quantization_sigma = match bytes[1] {
        0 => None,
        1 => {
            let sigma = f32::from_le_bytes(bytes[2..6].try_into().unwrap());
            if !sigma.is_finite() || sigma <= 0.0 {
                return None;
            }
            Some(sigma)
        }
        _ => return None,
    };
    let mut channels = ArrayVec::new();
    for i in 0..n {
        let off = 6 + i * 8;
        let median = f32::from_le_bytes(bytes[off..off + 4].try_into().unwrap());
        let mad = f32::from_le_bytes(bytes[off + 4..off + 8].try_into().unwrap());
        channels.push(ChannelStats { median, mad });
    }
    Some(FrameStats {
        channels,
        quantization_sigma,
    })
}

#[cfg(test)]
mod tests;
