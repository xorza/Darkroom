//! Tier selection, frame loading, and persistent cache sidecars.

use std::io::Error as IoError;
use std::mem::size_of;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use arrayvec::ArrayVec;
use common::CancelToken;
use common::file_utils;
use common::parallel::try_par_map_limited;

use crate::io::astro_image::cfa::CfaImage;
use crate::io::astro_image::{AstroImage, AstroImageMetadata, ImageDimensions};
use crate::math::statistics::ChannelStats;
use crate::stacking::combine::cache_config::CacheConfig;
use crate::stacking::combine::config::Normalization;
use crate::stacking::combine::error::Error;
use crate::stacking::combine::normalization::compute_frame_norms;
use crate::stacking::frame_store::{
    FrameStats, FrameStoreError, SpillDirectory, StackableImage, StoredFrame, StoredLightFrame,
    StoredPlane, cache_filename, channel_filename, compute_frame_stats, decode_transient_bytes,
    fits_in_memory, frame_bytes, frame_from_memory, load_concurrency, map_plane, reusable_plane,
    store_frame,
};
use crate::stacking::progress::{ProgressCallback, StackingStage, report_progress};

use crate::stacking::combine::cache::{CacheCore, CfaCache, LightCache};

#[derive(Debug)]
struct LoadedTier {
    frames: Vec<StoredFrame>,
    spill_directory: Option<SpillDirectory>,
    frame_stats: Vec<FrameStats>,
    metadata: AstroImageMetadata,
}

/// [`load_tiered`] output: the loaded plain frames plus the assembled [`CacheCore`].
#[derive(Debug)]
struct LoadedCache {
    frames: Vec<StoredFrame>,
    frame_stats: Vec<FrameStats>,
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

    report_progress(&progress, 0, paths.len(), StackingStage::Loading);

    let first_path = paths[0].as_ref();
    let available_memory = config.get_available_memory();

    // Dimensions drive the in-memory-vs-disk tier decision. Peek the header without a decode when
    // the format allows it (RAW), so the in-memory path can decode every frame in parallel rather
    // than decoding frame 0 serially first; otherwise decode frame 0 and reuse it below.
    let (dimensions, first_image) = match I::peek_dimensions(first_path) {
        Some(dims) => (dims, None),
        None => {
            let img = load_image::<I>(first_path)?;
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
        frame_stats,
        metadata,
    } = if use_in_memory {
        load_in_memory::<I, P>(
            paths,
            &progress,
            dimensions,
            first_image,
            available_memory,
            &cancel,
        )?
    } else {
        // Disk tier (large stacks): the serial-first-frame path. If the header was peeked we
        // haven't decoded frame 0 yet, so decode it now — rare, since calibration fits in RAM.
        let first = match first_image {
            Some(img) => img,
            None => load_image::<I>(first_path)?,
        };
        load_to_disk::<I, P>(
            paths,
            config,
            &progress,
            dimensions,
            first,
            available_memory,
            &cancel,
        )?
    };

    Ok(LoadedCache {
        frames,
        frame_stats,
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

fn load_image<I: StackableImage>(path: &Path) -> Result<I, Error> {
    I::load(path).map_err(|source| Error::ImageLoad {
        path: path.to_path_buf(),
        source: IoError::other(source),
    })
}

impl CfaCache {
    /// Build a calibration cache from CFA frame files (tiered in-memory/disk per available RAM).
    pub(crate) fn from_paths<P: AsRef<Path> + Sync>(
        paths: &[P],
        config: &CacheConfig,
        progress: ProgressCallback,
        cancel: CancelToken,
    ) -> Result<Self, Error> {
        let LoadedCache {
            frames,
            frame_stats,
            core,
        } = load_tiered::<CfaImage, P>(paths, config, progress, cancel)?;
        Ok(Self {
            frames,
            frame_stats,
            core,
        })
    }
}

impl LightCache {
    /// Build a light-frame cache from image files (tiered per available RAM). Disk files carry no
    /// warp quality planes, so every pixel has full support and unit confidence.
    pub(crate) fn from_paths<P: AsRef<Path> + Sync>(
        paths: &[P],
        config: &CacheConfig,
        normalization: Normalization,
        progress: ProgressCallback,
        cancel: CancelToken,
    ) -> Result<Self, Error> {
        let LoadedCache {
            frames,
            frame_stats,
            core,
        } = load_tiered::<AstroImage, P>(paths, config, progress, cancel)?;
        let frame_norms = compute_frame_norms(&frame_stats, normalization);
        let frames = frames
            .into_iter()
            .zip(frame_stats)
            .map(|(frame, stats)| StoredLightFrame::from_stored(frame, stats))
            .collect();
        Ok(Self {
            frames,
            frame_norms,
            core,
        })
    }
}

#[derive(Debug)]
struct LoadedMemoryFrame {
    frame: StoredFrame,
    stats: FrameStats,
    metadata: Option<AstroImageMetadata>,
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
    cancel: &CancelToken,
) -> Result<LoadedTier, Error> {
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
    let indexed_paths: Vec<(usize, &P)> = paths[start..]
        .iter()
        .enumerate()
        .map(|(i, p)| (i + start, p))
        .collect();
    let loaded = try_par_map_limited(&indexed_paths, concurrency, |&(idx, path)| {
        // Cancelled: stop decoding further frames (the slow phase).
        if cancel.is_cancelled() {
            return Err(Error::Cancelled);
        }
        let image = load_image::<I>(path.as_ref())?;
        if image.dimensions() != dimensions {
            return Err(Error::DimensionMismatch {
                index: idx,
                expected: dimensions,
                actual: image.dimensions(),
            });
        }
        let metadata = (idx == 0).then(|| image.metadata().clone());
        let stats = compute_frame_stats(&image);
        Ok(LoadedMemoryFrame {
            frame: frame_from_memory(image),
            stats,
            metadata,
        })
    })?;

    let mut frames = Vec::with_capacity(paths.len());
    let mut all_stats = Vec::with_capacity(paths.len());
    let mut metadata = None;
    if let Some(first_image) = first {
        metadata = Some(first_image.metadata().clone());
        all_stats.push(compute_frame_stats(&first_image));
        frames.push(frame_from_memory(first_image));
    }
    for loaded_frame in loaded {
        if loaded_frame.metadata.is_some() {
            metadata = loaded_frame.metadata;
        }
        frames.push(loaded_frame.frame);
        all_stats.push(loaded_frame.stats);
    }

    report_progress(progress, paths.len(), paths.len(), StackingStage::Loading);

    tracing::info!("Loaded {} frames into memory", frames.len());
    Ok(LoadedTier {
        frames,
        spill_directory: None,
        frame_stats: all_stats,
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
    cancel: &CancelToken,
) -> Result<LoadedTier, Error> {
    let spill_directory = SpillDirectory::create(config.cache_dir.clone(), config.keep_cache)?;
    let cache_dir = &spill_directory.path;

    // Cache first image and compute stats. Frame 0 carries the stack metadata.
    let metadata = first_image.metadata().clone();
    let first_stats = compute_frame_stats(&first_image);
    let first_path = paths[0].as_ref();
    let base_filename = cache_filename(first_path);
    let first_cached = store_frame(cache_dir, &base_filename, &first_image).map_err(Error::from)?;
    report_progress(progress, 1, paths.len(), StackingStage::Loading);

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
    let indexed_paths: Vec<(usize, &P)> = paths[1..]
        .iter()
        .enumerate()
        .map(|(i, p)| (i + 1, p))
        .collect();
    let remaining = try_par_map_limited(&indexed_paths, concurrency, |&(idx, ref path)| {
        // Cancelled: stop decoding further frames (the slow phase).
        if cancel.is_cancelled() {
            return Err(Error::Cancelled);
        }
        let path_ref = path.as_ref();
        let base_filename = cache_filename(path_ref);
        load_and_cache_frame::<I>(cache_dir, &base_filename, path_ref, dimensions, idx)
    })?;

    // Build final vectors
    let mut frames = Vec::with_capacity(paths.len());
    let mut all_stats = Vec::with_capacity(paths.len());
    frames.push(first_cached);
    all_stats.push(first_stats);
    for loaded in remaining {
        frames.push(loaded.frame);
        all_stats.push(loaded.stats);
    }

    report_progress(progress, paths.len(), paths.len(), StackingStage::Loading);

    tracing::info!(
        "Cached {} frames ({} channels each) to disk at {:?}",
        frames.len(),
        dimensions.channels(),
        cache_dir
    );

    Ok(LoadedTier {
        frames,
        spill_directory: Some(spill_directory),
        frame_stats: all_stats,
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
#[derive(Debug)]
struct LoadedStoredFrame {
    frame: StoredFrame,
    stats: FrameStats,
}

fn load_and_cache_frame<I: StackableImage>(
    cache_dir: &Path,
    base_filename: &str,
    source_path: &Path,
    dimensions: ImageDimensions,
    frame_index: usize,
) -> Result<LoadedStoredFrame, Error> {
    let channels = dimensions.channels();
    let identity_before = source_identity(source_path)?;

    // Check if all channel files exist, have correct size, and source hasn't changed
    let meta_valid = validate_source_meta(cache_dir, base_filename, &identity_before);
    let has_stats = stats_path(cache_dir, base_filename).exists();
    let can_reuse = meta_valid
        && has_stats
        && (0..channels).all(|c| {
            let channel_path = cache_dir.join(channel_filename(base_filename, c));
            reusable_plane(&channel_path, dimensions)
        });

    if can_reuse {
        // Reuse existing cache files - just mmap them
        let mut planes = ArrayVec::new();
        for c in 0..channels {
            let channel_path = cache_dir.join(channel_filename(base_filename, c));
            planes.push(StoredPlane::Mapped(map_plane(channel_path)?));
        }
        tracing::debug!(
            source = %source_path.display(),
            "Reusing existing cache files"
        );
        let frame = StoredFrame { channels: planes };
        let stats = read_frame_stats(cache_dir, base_filename)
            .expect("stats sidecar missing for valid cache");
        Ok(LoadedStoredFrame { frame, stats })
    } else {
        // Load image and write to cache
        let image = load_image::<I>(source_path)?;

        if image.dimensions() != dimensions {
            return Err(Error::DimensionMismatch {
                index: frame_index,
                expected: dimensions,
                actual: image.dimensions(),
            });
        }
        let identity_after = source_identity(source_path)?;
        if identity_after != identity_before {
            return Err(FrameStoreError::SourceChanged {
                path: source_path.to_path_buf(),
            }
            .into());
        }

        let stats = compute_frame_stats(&image);
        let result = store_frame(cache_dir, base_filename, &image).map_err(Error::from)?;

        // The identity sidecar is the commit record for the planes and stats.
        write_frame_stats(cache_dir, base_filename, &stats)?;
        write_source_meta(cache_dir, base_filename, &identity_after)?;

        Ok(LoadedStoredFrame {
            frame: result,
            stats,
        })
    }
}

/// Path for the sidecar stats file.
fn stats_path(cache_dir: &Path, base_filename: &str) -> PathBuf {
    cache_dir.join(format!("{}.stats", base_filename.trim_end_matches(".bin")))
}

/// Write frame stats to a sidecar file.
/// Format: [n_channels: u8] [median_0: f32] [mad_0: f32] [median_1: f32] ...
fn write_frame_stats(
    cache_dir: &Path,
    base_filename: &str,
    stats: &FrameStats,
) -> Result<(), FrameStoreError> {
    let path = stats_path(cache_dir, base_filename);
    let n = stats.channels.len();
    let mut buf = Vec::with_capacity(1 + n * 8);
    buf.push(n as u8);
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
    if bytes.is_empty() {
        return None;
    }
    let n = bytes[0] as usize;
    if bytes.len() != 1 + n * 8 {
        return None;
    }
    let mut channels = ArrayVec::new();
    for i in 0..n {
        let off = 1 + i * 8;
        let median = f32::from_le_bytes(bytes[off..off + 4].try_into().unwrap());
        let mad = f32::from_le_bytes(bytes[off + 4..off + 8].try_into().unwrap());
        channels.push(ChannelStats { median, mad });
    }
    Some(FrameStats { channels })
}

#[cfg(test)]
mod tests;
