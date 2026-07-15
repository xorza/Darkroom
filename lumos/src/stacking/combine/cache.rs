//! Image cache for stacking operations.
//!
//! Frames are stored as planar [`Plane`]s — each plane either in RAM ([`Plane::Memory`]) or
//! memory-mapped from disk ([`Plane::Mapped`]), chosen per stack by whether the set fits in ~75% of
//! RAM. One chunked-read path ([`Plane::chunk`]) serves both tiers, and a frame's planes always
//! share a tier.
//!
//! Two concrete caches, one per image type, sharing the tiered store + combine engine
//! ([`CacheCore`]) by composition:
//! - [`CfaCache`] — `CfaImage` calibration frames, [`Frame`]s with **no coverage at all**, plain
//!   combine → `CfaImage`.
//! - [`LightCache`] — `AstroImage` light frames, [`WeightedFrame`]s with optional per-pixel
//!   coverage, coverage-weighted combine → [`StackProduct`].
//!
//! Coverage is type-true: it exists only on [`WeightedFrame`] (so [`CfaCache`] cannot carry it),
//! and a frame's planes always share a tier, so coverage is memory-mapped exactly when its
//! channels are.
//!
//! Disk cache format: one file per channel, `{hash}_c{channel}.bin`, raw f32 row-major
//! (width * height * 4 bytes); loading is shared across image types via [`StackableImage`].

use std::fs::File;
use std::hash::{Hash, Hasher};
use std::io::{BufWriter, Write};
use std::mem::size_of;
use std::path::{Path, PathBuf};

use arrayvec::ArrayVec;
use common::CancelToken;
use common::FnvHasher;
use common::parallel::try_par_map_limited;
use imaginarium::Buffer2;
use memmap2::Mmap;
use rayon::prelude::*;

use crate::io::astro_image::cfa::CfaImage;
use crate::io::astro_image::{AstroImage, AstroImageMetadata, ImageDimensions, PixelData};
use crate::math::statistics::{ChannelStats, mad_f32_with_scratch, median_f32_mut};
use crate::stacking::combine::cache_config::{
    CacheConfig, compute_load_concurrency, compute_optimal_chunk_rows_with_memory, fits_in_memory,
};
use crate::stacking::combine::error::Error;
use crate::stacking::combine::progress::{ProgressCallback, StackingStage, report_progress};
use crate::stacking::combine::stack::{FrameNorm, StackFrame};
use crate::stacking::product::StackProduct;

/// Per-frame statistics: one `ChannelStats` per channel.
#[derive(Debug, Clone)]
pub(crate) struct FrameStats {
    pub channels: ArrayVec<ChannelStats, 3>,
}

/// Compute per-channel median + MAD for a single image.
fn compute_frame_stats(image: &impl StackableImage) -> FrameStats {
    let dims = image.dimensions();
    let n = dims.channels;

    if n == 1 {
        // Single channel — no parallelism needed, avoid rayon overhead.
        let data = image.channel(0);
        let mut buf = Vec::with_capacity(dims.size.x * dims.size.y);
        buf.extend_from_slice(data);
        let median = median_f32_mut(&mut buf);
        let mad = mad_f32_with_scratch(data, median, &mut buf);
        let mut channels = ArrayVec::new();
        channels.push(ChannelStats { median, mad });
        return FrameStats { channels };
    }

    // Multi-channel: compute stats in parallel.
    let results: Vec<ChannelStats> = (0..n)
        .into_par_iter()
        .map(|c| {
            let data = image.channel(c);
            let mut buf = Vec::with_capacity(dims.size.x * dims.size.y);
            buf.extend_from_slice(data);
            let median = median_f32_mut(&mut buf);
            let mad = mad_f32_with_scratch(data, median, &mut buf);
            ChannelStats { median, mad }
        })
        .collect();

    FrameStats {
        channels: results.into_iter().collect(),
    }
}

/// Per-thread scratch buffers for stacking combine closures.
///
/// Allocated once per rayon thread via `for_each_init` and reused across all pixels.
#[derive(Debug)]
pub(crate) struct ScratchBuffers {
    /// Tracks original frame indices after rejection reordering.
    pub indices: Vec<usize>,
    /// General-purpose f32 scratch (e.g. winsorized working copy).
    pub floats_a: Vec<f32>,
    /// Second f32 scratch (e.g. median/MAD computation).
    pub floats_b: Vec<f32>,
    /// usize scratch (large-N `sort_with_indices` permutation).
    pub usize_a: Vec<usize>,
    /// Second usize scratch (large-N `sort_with_indices` index copy).
    pub usize_b: Vec<usize>,
}

impl ScratchBuffers {
    fn new(frame_count: usize) -> Self {
        Self {
            indices: Vec::with_capacity(frame_count),
            floats_a: Vec::with_capacity(frame_count),
            floats_b: Vec::with_capacity(frame_count),
            usize_a: Vec::with_capacity(frame_count),
            usize_b: Vec::with_capacity(frame_count),
        }
    }
}

/// Trait for images that can be loaded into a stacking cache ([`CfaCache`]/[`LightCache`]).
///
/// Implementations must provide planar channel access as `&[f32]` slices
/// and a file-loading constructor.
pub(crate) trait StackableImage: Send + Sync + std::fmt::Debug + Sized {
    fn dimensions(&self) -> ImageDimensions;
    fn channel(&self, c: usize) -> &[f32];
    fn metadata(&self) -> &AstroImageMetadata;
    fn load(path: &Path) -> Result<Self, Error>;

    /// Pixel dimensions from the file header alone, *without* a full decode, when the format allows
    /// it. Lets the loader size its tier and decode every frame in parallel instead of decoding
    /// frame 0 serially just for its dimensions. `None` (the default) → the loader falls back to a
    /// full first-frame decode.
    fn peek_dimensions(_path: &Path) -> Option<ImageDimensions> {
        None
    }

    /// Consume the image, moving its channel planes out (no copy) — one `Buffer2` per channel in
    /// channel order. Populates the in-memory cache tier as [`Plane::Memory`].
    fn into_planes(self) -> ArrayVec<Buffer2<f32>, 3>;

    /// Construct from stacking output.
    fn from_stacked(
        pixels: PixelData,
        metadata: AstroImageMetadata,
        dimensions: ImageDimensions,
    ) -> Self;
}

/// In-memory byte footprint of one frame's pixel data (planes are `f32`). Drives the tier decision
/// and (as the resident-set term) the decode-concurrency budget.
fn frame_bytes(dimensions: ImageDimensions) -> usize {
    dimensions.sample_count() * size_of::<f32>()
}

/// Peak transient heap one in-flight decode holds, relative to the decoded frame. This — not the
/// resident [`frame_bytes`] — is what [`compute_load_concurrency`] must divide the memory headroom
/// by; sizing against the smaller resident frame let peak load heap overshoot the budget ~2×.
///
/// The factor is 2× and format-independent, because the dominant term is shared: every loader hands
/// its decoded f32 frame (1×) to [`compute_frame_stats`], which copies an equal-size median scratch
/// (1×) before the frame is stored or spilled. Both concrete loaders peak at exactly that window —
/// FITS reads the source bytes but drops them before stats; the libraw CFA path reads a borrowed
/// `&[u16]` (no copy) and frees libraw's buffers before stats — so neither exceeds the 2× floor.
/// (The RAW decode→demosaic step in `pipeline` holds a much larger arena and budgets its own
/// transient separately; it does not go through this loader.)
const DECODE_TRANSIENT_FACTOR: usize = 2;

fn decode_transient_bytes(dimensions: ImageDimensions) -> usize {
    DECODE_TRANSIENT_FACTOR * frame_bytes(dimensions)
}

/// Generate a cache filename from the hash of the source path.
fn cache_filename_for_path(path: &Path) -> String {
    let mut hasher = FnvHasher::new();
    path.hash(&mut hasher);
    let hash = hasher.finish();
    format!("{:016x}.bin", hash)
}

/// One W×H f32 plane — in RAM or memory-mapped. The single tiering primitive: every stored
/// frame's channels (and a warped frame's coverage) is a `Plane`, so the in-memory and disk tiers
/// share one chunked-read path and a frame's planes are always the same tier.
#[derive(Debug)]
pub(crate) enum Plane {
    Memory(Buffer2<f32>),
    Mapped(Mmap),
}

impl Plane {
    /// Pixels `start..end` (a row-aligned chunk) as `f32`, tier-agnostic.
    #[inline]
    fn chunk(&self, start: usize, end: usize) -> &[f32] {
        match self {
            Plane::Memory(buffer) => &buffer[start..end],
            Plane::Mapped(mmap) => {
                bytemuck::cast_slice(&mmap[start * size_of::<f32>()..end * size_of::<f32>()])
            }
        }
    }
}

/// A stored frame's channel planes — implemented by both [`Frame`] and [`WeightedFrame`] so the
/// chunked combine machinery is shared. Coverage is deliberately *not* here: it lives only on
/// [`WeightedFrame`], so a plain stack's frame type carries no coverage at all.
pub(crate) trait CacheFrame: Send + Sync {
    fn channels(&self) -> &[Plane];
}

/// Plain stack frame: channels only, never coverage. Used for CFA masters and plain light stacks.
#[derive(Debug)]
pub(crate) struct Frame {
    channels: ArrayVec<Plane, 3>,
}

impl CacheFrame for Frame {
    fn channels(&self) -> &[Plane] {
        &self.channels
    }
}

/// Registered/warped stack frame: channels + optional per-pixel coverage (`None` = fully covered,
/// e.g. an unwarped reference). All of a frame's planes share one tier, so coverage is
/// memory-mapped exactly when its channels are.
#[derive(Debug)]
pub(crate) struct WeightedFrame {
    channels: ArrayVec<Plane, 3>,
    coverage: Option<Plane>,
}

impl CacheFrame for WeightedFrame {
    fn channels(&self) -> &[Plane] {
        &self.channels
    }
}

/// Shared cache context + combine engine — everything that doesn't depend on the frame type.
/// Owned by composition inside [`CfaCache`] and [`LightCache`]; all frames share one tier, and
/// `cache_dir` is `Some` only when the planes are memory-mapped (removed on cleanup/drop).
#[derive(Debug)]
pub(crate) struct CacheCore {
    pub(crate) cache_dir: Option<PathBuf>,
    /// Image dimensions (same for all frames).
    pub(crate) dimensions: ImageDimensions,
    /// Metadata from the first frame.
    pub(crate) metadata: AstroImageMetadata,
    /// Per-frame channel statistics (median + MAD), computed at load time.
    pub(crate) channel_stats: Vec<FrameStats>,
    /// Configuration for cache operations.
    pub(crate) config: CacheConfig,
    /// Progress callback.
    pub(crate) progress: ProgressCallback,
    /// Cooperative cancel flag, polled by [`Self::process_chunks`] between
    /// chunks. `None` (the construction default) = never cancel; a public
    /// stacking entry sets it from the caller's flag before the combine.
    pub(crate) cancel: CancelToken,
}

/// Plain calibration cache: `CfaImage` frames, channels only (never coverage), plain combine.
/// Built from disk via [`CfaCache::from_paths`]; stacks to a `CfaImage`.
#[derive(Debug)]
pub(crate) struct CfaCache {
    // Declared before `core` so the frames' mmaps drop before `CacheCore`'s `Drop` removes the dir.
    pub(crate) frames: Vec<Frame>,
    pub(crate) core: CacheCore,
}

/// Light-frame stacking cache: `AstroImage` frames with optional per-pixel coverage, coverage-
/// weighted combine. Built in-memory from [`StackFrame`]s ([`LightCache::from_stack_frames`]) or
/// from disk ([`LightCache::from_paths`], coverage `None`); stacks to an `AstroImage`.
#[derive(Debug)]
pub(crate) struct LightCache {
    pub(crate) frames: Vec<WeightedFrame>,
    pub(crate) core: CacheCore,
}

/// Tier-loader output: the stored [`Frame`]s plus the disk cache dir (`Some` if memory-mapped),
/// per-frame stats, and frame 0's metadata. Assembled into a [`CacheCore`] by [`load_tiered`].
struct LoadedTier {
    frames: Vec<Frame>,
    cache_dir: Option<PathBuf>,
    channel_stats: Vec<FrameStats>,
    metadata: AstroImageMetadata,
}

/// [`load_tiered`] output: the loaded plain frames plus the assembled [`CacheCore`].
struct LoadedCache {
    frames: Vec<Frame>,
    core: CacheCore,
}

/// Minimum coverage for a warped frame to contribute at a pixel. Below this the frame's value
/// there is (near-)entirely warp border-fill, so it's *excluded* from the combine rather than
/// down-weighted — otherwise its fill would skew rejection and re-introduce the dark warped-edge
/// ring this weighting exists to remove.
const COVERAGE_EPSILON: f32 = 1e-3;

/// Per-chunk context handed to the [`CacheCore::process_chunks`] closure: the input frame
/// slices for this chunk plus the geometry to map a within-chunk pixel to a global frame index.
#[derive(Debug)]
struct ChunkContext<'a> {
    /// One channel slice per frame for this chunk; `frames.len()` is the frame count.
    frames: &'a [&'a [f32]],
    /// Row width in pixels.
    width: usize,
    /// Channel currently being combined.
    channel: usize,
    /// Global pixel index of this chunk's first pixel — for indexing full-frame,
    /// channel-independent maps such as coverage.
    pixel_offset: usize,
}

/// Load `paths` into tiered plain [`Frame`]s plus an assembled [`CacheCore`], choosing in-memory vs
/// disk-backed (mmap) storage by whether the set fits ~75% of RAM. Shared by both caches'
/// `from_paths`; the image type `I` only governs decoding.
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
            let img = I::load(first_path)?;
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
        cache_dir,
        channel_stats,
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
            None => I::load(first_path)?,
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
        core: CacheCore {
            cache_dir,
            dimensions,
            metadata,
            channel_stats,
            config: config.clone(),
            progress,
            cancel,
        },
    })
}

impl CfaCache {
    /// Build a calibration cache from CFA frame files (tiered in-memory/disk per available RAM).
    pub fn from_paths<P: AsRef<Path> + Sync>(
        paths: &[P],
        config: &CacheConfig,
        progress: ProgressCallback,
        cancel: CancelToken,
    ) -> Result<Self, Error> {
        let LoadedCache { frames, core } =
            load_tiered::<CfaImage, P>(paths, config, progress, cancel)?;
        Ok(Self { frames, core })
    }
}

impl LightCache {
    /// Build a light-frame cache from image files (tiered per available RAM). These carry no
    /// coverage (`None`) — disk files don't store it — so the weighted combine treats every pixel
    /// as fully covered, matching a plain stack.
    pub fn from_paths<P: AsRef<Path> + Sync>(
        paths: &[P],
        config: &CacheConfig,
        progress: ProgressCallback,
        cancel: CancelToken,
    ) -> Result<Self, Error> {
        let LoadedCache { frames, core } =
            load_tiered::<AstroImage, P>(paths, config, progress, cancel)?;
        let frames = frames
            .into_iter()
            .map(|frame| WeightedFrame {
                channels: frame.channels,
                coverage: None,
            })
            .collect();
        Ok(Self { frames, core })
    }
}

/// Move a loaded image's channels into an in-memory [`Frame`] (no copy — the channel buffers are
/// moved out of the image and wrapped as [`Plane::Memory`]).
fn image_to_frame<I: StackableImage>(image: I) -> Frame {
    Frame {
        channels: image.into_planes().into_iter().map(Plane::Memory).collect(),
    }
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
    let concurrency = compute_load_concurrency(
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
        let image = I::load(path.as_ref())?;
        if image.dimensions() != dimensions {
            return Err(Error::DimensionMismatch {
                index: idx,
                expected: dimensions,
                actual: image.dimensions(),
            });
        }
        let metadata = (idx == 0).then(|| image.metadata().clone());
        let stats = compute_frame_stats(&image);
        Ok((image_to_frame(image), stats, metadata))
    })?;

    let mut frames = Vec::with_capacity(paths.len());
    let mut all_stats = Vec::with_capacity(paths.len());
    let mut metadata = None;
    if let Some(first_image) = first {
        metadata = Some(first_image.metadata().clone());
        all_stats.push(compute_frame_stats(&first_image));
        frames.push(image_to_frame(first_image));
    }
    for (frame, stats, meta) in loaded {
        if meta.is_some() {
            metadata = meta;
        }
        frames.push(frame);
        all_stats.push(stats);
    }

    report_progress(progress, paths.len(), paths.len(), StackingStage::Loading);

    tracing::info!("Loaded {} frames into memory", frames.len());
    Ok(LoadedTier {
        frames,
        cache_dir: None,
        channel_stats: all_stats,
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
    let cache_dir = &config.cache_dir;
    std::fs::create_dir_all(cache_dir).map_err(|e| Error::CreateCacheDir {
        path: cache_dir.to_path_buf(),
        source: e,
    })?;

    // Cache first image and compute stats. Frame 0 carries the stack metadata.
    let metadata = first_image.metadata().clone();
    let first_stats = compute_frame_stats(&first_image);
    let first_path = paths[0].as_ref();
    let base_filename = cache_filename_for_path(first_path);
    let first_cached = cache_image_channels(cache_dir, &base_filename, &first_image, dimensions)?;
    report_progress(progress, 1, paths.len(), StackingStage::Loading);

    // Decode is CPU-bound, so fan out to the worker count, bounded by RAM. The disk tier streams
    // each decoded frame to its own file and drops it, so nothing stays resident (`0`) — only the
    // in-flight decodes occupy memory, each its true ~2× transient. Each frame writes unique files,
    // so there's no contention.
    let concurrency = compute_load_concurrency(
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
        let base_filename = cache_filename_for_path(path_ref);
        load_and_cache_frame::<I>(cache_dir, &base_filename, path_ref, dimensions, idx)
    })?;

    // Build final vectors
    let mut frames = Vec::with_capacity(paths.len());
    let mut all_stats = Vec::with_capacity(paths.len());
    frames.push(first_cached);
    all_stats.push(first_stats);
    for (frame, stats) in remaining {
        frames.push(frame);
        all_stats.push(stats);
    }

    report_progress(progress, paths.len(), paths.len(), StackingStage::Loading);

    tracing::info!(
        "Cached {} frames ({} channels each) to disk at {:?}",
        frames.len(),
        dimensions.channels,
        cache_dir
    );

    Ok(LoadedTier {
        frames,
        cache_dir: Some(cache_dir.to_path_buf()),
        channel_stats: all_stats,
        metadata,
    })
}

impl CacheCore {
    /// Combine engine: walk the output in memory-bounded row chunks (whole planes for in-memory
    /// stacks, bounded row chunks for disk-backed), gather each frame's channel slice for the
    /// chunk via [`Plane::chunk`], and hand `(output_slice, ChunkContext)` to `process`. The frames
    /// live in the owning cache, so they're passed in. Returns the combined `PixelData`.
    fn process_chunks<F, Process>(&self, frames: &[F], mut process: Process) -> PixelData
    where
        F: CacheFrame,
        Process: FnMut(&mut [f32], ChunkContext),
    {
        let dims = self.dimensions;
        let frame_count = frames.len();
        let width = dims.size.x;
        let height = dims.size.y;

        // Whole-plane chunks in RAM; for disk-backed stacks size chunks to the memory budget
        // (queried only here, where it's used — an in-memory stack skips the sysinfo read).
        let chunk_rows = if self.cache_dir.is_none() {
            height
        } else {
            let available_memory = self.config.get_available_memory();
            compute_optimal_chunk_rows_with_memory(width, 1, frame_count, available_memory)
        };

        let mut output = PixelData::new_default(width, height, dims.channels);
        let channels = output.channels();

        let num_chunks = height.div_ceil(chunk_rows);
        let total_work = num_chunks * channels;

        let mut chunks: Vec<&[f32]> = Vec::with_capacity(frame_count);

        report_progress(&self.progress, 0, total_work, StackingStage::Processing);

        for channel in 0..channels {
            for chunk_idx in 0..num_chunks {
                let start_row = chunk_idx * chunk_rows;
                let end_row = (start_row + chunk_rows).min(height);
                let rows_in_chunk = end_row - start_row;
                let pixels_in_chunk = rows_in_chunk * width;

                chunks.clear();
                chunks.extend((0..frame_count).map(|frame_idx| {
                    self.read_channel_chunk(frames, frame_idx, channel, start_row, end_row)
                }));

                let output_slice = &mut output.channel_mut(channel).pixels_mut()
                    [start_row * width..][..pixels_in_chunk];

                process(
                    output_slice,
                    ChunkContext {
                        frames: &chunks,
                        width,
                        channel,
                        pixel_offset: start_row * width,
                    },
                );

                report_progress(
                    &self.progress,
                    channel * num_chunks + chunk_idx + 1,
                    total_work,
                    StackingStage::Processing,
                );

                // Cooperative cancel: bail between chunks (the in-flight chunk
                // completes). The partial `output` is discarded by the caller,
                // which detects the cancel and returns `Error::Cancelled`.
                if self.cancel.is_cancelled() {
                    return output;
                }
            }
        }

        output
    }

    /// Read a horizontal chunk (rows `start_row..end_row`) of a single channel from one frame,
    /// tier-agnostically via [`Plane::chunk`].
    fn read_channel_chunk<'a, F: CacheFrame>(
        &self,
        frames: &'a [F],
        frame_idx: usize,
        channel: usize,
        start_row: usize,
        end_row: usize,
    ) -> &'a [f32] {
        let width = self.dimensions.size.x;
        frames[frame_idx].channels()[channel].chunk(start_row * width, end_row * width)
    }

    /// Remove the disk cache directory, if any (no-op for in-memory stacks or when `keep_cache`).
    pub fn cleanup(&self) {
        if self.config.keep_cache {
            return;
        }
        if let Some(cache_dir) = &self.cache_dir {
            let _ = std::fs::remove_dir_all(cache_dir);
        }
    }
}

impl Drop for CacheCore {
    fn drop(&mut self) {
        self.cleanup();
    }
}

impl CfaCache {
    /// Plain per-pixel combine: every frame contributes at every pixel (CFA frames have no
    /// coverage). Optional `weights` provide per-frame weights; `frame_norms` apply per-frame
    /// affine normalization before combining. Per-channel, parallelized per-row with rayon.
    pub fn process_chunked<Combine>(
        &self,
        weights: Option<&[f32]>,
        frame_norms: Option<&[FrameNorm]>,
        combine: Combine,
    ) -> PixelData
    where
        Combine: Fn(&mut [f32], Option<&[f32]>, &mut ScratchBuffers) -> f32 + Sync,
    {
        if let Some(w) = weights {
            assert_eq!(
                w.len(),
                self.frames.len(),
                "Weight count must match frame count"
            );
        }
        // An in-memory stack is one chunk, so the per-chunk cancel check in
        // `process_chunks` can't interrupt the combine — poll per row here too.
        let cancel = self.core.cancel.clone();
        self.core.process_chunks(&self.frames, |output_slice, ctx| {
            let ChunkContext {
                frames,
                width,
                channel,
                pixel_offset: _,
            } = ctx;
            let frame_count = frames.len();
            output_slice
                .par_chunks_mut(width)
                .enumerate()
                .for_each_init(
                    || (vec![0.0f32; frame_count], ScratchBuffers::new(frame_count)),
                    |(values, scratch), (row_in_chunk, row_output)| {
                        // Cancelled: skip the row's work (output stays zero; the
                        // caller discards the partial result and reports Cancelled).
                        if cancel.is_cancelled() {
                            return;
                        }
                        let row_offset = row_in_chunk * width;
                        for (pixel_in_row, out) in row_output.iter_mut().enumerate() {
                            let pixel_idx = row_offset + pixel_in_row;
                            if let Some(frame_norm) = frame_norms {
                                for (frame_idx, chunk) in frames.iter().enumerate() {
                                    let cn = frame_norm[frame_idx].channels[channel];
                                    values[frame_idx] = chunk[pixel_idx] * cn.gain + cn.offset;
                                }
                            } else {
                                for (frame_idx, chunk) in frames.iter().enumerate() {
                                    values[frame_idx] = chunk[pixel_idx];
                                }
                            }
                            // The combine reducers (median/MAD, compensated sums) assume finite
                            // inputs — NaN/Inf would silently corrupt the output (NaN-unsafe
                            // ordering, multiply-through). No production path emits them today
                            // (FITS load sanitizes, flat division is floored, warp border-fills 0),
                            // so this guards against a future upstream regression.
                            debug_assert!(
                                values.iter().all(|v| v.is_finite()),
                                "non-finite pixel value entered the combine",
                            );
                            *out = combine(values, weights, scratch);
                        }
                    },
                );
        })
    }
}

impl LightCache {
    /// Build a cache from pre-tiered [`WeightedFrame`]s and their stats — the streaming pipeline
    /// produces these directly (spilling to disk or keeping in RAM as it warps), so the whole frame
    /// set is never materialised at once. `cache_dir` is `Some` for the disk tier (removed on
    /// `Drop`), `None` for the RAM tier. `dimensions`/`metadata` come from the (consumed) frames.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn from_weighted_frames(
        frames: Vec<WeightedFrame>,
        channel_stats: Vec<FrameStats>,
        cache_dir: Option<PathBuf>,
        dimensions: ImageDimensions,
        metadata: AstroImageMetadata,
        config: &CacheConfig,
        progress: ProgressCallback,
        cancel: CancelToken,
    ) -> Self {
        debug_assert_eq!(frames.len(), channel_stats.len());
        Self {
            frames,
            core: CacheCore {
                cache_dir,
                dimensions,
                metadata,
                channel_stats,
                config: config.clone(),
                progress,
                cancel,
            },
        }
    }

    /// Build an in-memory coverage-weighted cache from [`StackFrame`]s (image + optional
    /// per-pixel coverage), moving channels and coverage into [`Plane::Memory`] (no copy).
    pub fn from_stack_frames(
        frames: Vec<StackFrame>,
        config: &CacheConfig,
        progress: ProgressCallback,
    ) -> Result<Self, Error> {
        if frames.is_empty() {
            return Err(Error::NoFrames);
        }
        let dimensions = frames[0].image.dimensions();
        let metadata = frames[0].image.metadata().clone();

        for (index, frame) in frames.iter().enumerate() {
            if index > 0 && frame.image.dimensions() != dimensions {
                return Err(Error::DimensionMismatch {
                    index,
                    expected: dimensions,
                    actual: frame.image.dimensions(),
                });
            }
            if let Some(coverage) = &frame.coverage
                && (coverage.width(), coverage.height()) != (dimensions.size.x, dimensions.size.y)
            {
                return Err(Error::CoverageDimensionMismatch {
                    index,
                    expected_width: dimensions.size.x,
                    expected_height: dimensions.size.y,
                    actual_width: coverage.width(),
                    actual_height: coverage.height(),
                });
            }
        }
        let channel_stats = frames
            .par_iter()
            .map(|frame| compute_frame_stats(&frame.image))
            .collect();
        let stored = frames
            .into_iter()
            .map(|frame| WeightedFrame {
                channels: frame
                    .image
                    .into_planes()
                    .into_iter()
                    .map(Plane::Memory)
                    .collect(),
                coverage: frame.coverage.map(Plane::Memory),
            })
            .collect();

        Ok(Self {
            frames: stored,
            core: CacheCore {
                cache_dir: None,
                dimensions,
                metadata,
                channel_stats,
                config: config.clone(),
                progress,
                // Set by the public stacking entry before the combine, if any.
                cancel: CancelToken::never(),
            },
        })
    }

    /// Geometric per-pixel quality planes — `coverage` (fraction of frames contributing, `[0,1]`),
    /// `weight` (`Σ wᵢcᵢ`, the WHT), and `variance` (`Σ(wᵢcᵢ)²/(Σ wᵢcᵢ)²`, output variance per unit
    /// input variance) — matching the `Σwᵢ`/`Σwᵢ²` maps drizzle reports. The weight `wᵢ·cᵢ`
    /// (per-frame weight × per-pixel coverage) doesn't depend on channel, so this rides cheaply on
    /// data the combine already touches and stays one plane each.
    ///
    /// **Pre-rejection:** a frame counts wherever its coverage clears [`COVERAGE_EPSILON`] — the set
    /// the combine draws from *before* outlier rejection narrows it per channel. Computed in a
    /// standalone pass so the combine engine is untouched; under aggressive rejection the variance
    /// is a slight under-estimate (see the per-channel follow-up in `docs/pipeline/roadmap.md`).
    ///
    /// `weights` are the same per-frame weights the combine used (`None` = equal, e.g. median).
    pub(crate) fn finish_product(
        &self,
        image: AstroImage,
        weights: Option<&[f32]>,
    ) -> StackProduct {
        let frame_count = self.frames.len();
        if let Some(w) = weights {
            assert_eq!(w.len(), frame_count, "weight count must match frame count");
        }
        let width = self.core.dimensions.size.x;
        let height = self.core.dimensions.size.y;
        let frame_weight = |f: usize| weights.map_or(1.0, |w| w[f]);

        // No frame carries a coverage map → every pixel is fully covered and the planes are
        // spatially uniform; fill constants and skip the per-pixel pass. (`stack()`-from-disk
        // always lands here, as does an all-reference in-memory stack.)
        if self.frames.iter().all(|f| f.coverage.is_none()) {
            let wsum: f32 = (0..frame_count).map(frame_weight).sum();
            let wsq: f32 = (0..frame_count).map(|f| frame_weight(f).powi(2)).sum();
            let variance = if wsum > 0.0 { wsq / (wsum * wsum) } else { 0.0 };
            return StackProduct {
                image,
                coverage: Buffer2::new_filled(width, height, 1.0),
                weight: Buffer2::new_filled(width, height, wsum),
                variance: Buffer2::new_filled(width, height, variance),
            };
        }

        let mut coverage = Buffer2::new_default(width, height);
        let mut weight = Buffer2::new_default(width, height);
        let mut variance = Buffer2::new_default(width, height);
        let inv_frames = 1.0 / frame_count as f32;

        // Coverage planes share their frame's tier, so they may be mmap-backed: read them in the
        // same row-aligned chunks the combine uses. The output planes are small single-channel RAM
        // buffers, written in parallel across the three at once, one row per task.
        let chunk_rows = if self.core.cache_dir.is_none() {
            height
        } else {
            compute_optimal_chunk_rows_with_memory(
                width,
                1,
                frame_count,
                self.core.config.get_available_memory(),
            )
        };

        let mut start_row = 0;
        while start_row < height {
            let end_row = (start_row + chunk_rows).min(height);
            let base = start_row * width;
            let span = (end_row - start_row) * width;

            let cov_chunks: Vec<Option<&[f32]>> = self
                .frames
                .iter()
                .map(|f| f.coverage.as_ref().map(|p| p.chunk(base, base + span)))
                .collect();

            let cov_out = &mut coverage.pixels_mut()[base..base + span];
            let wt_out = &mut weight.pixels_mut()[base..base + span];
            let var_out = &mut variance.pixels_mut()[base..base + span];
            cov_out
                .par_chunks_mut(width)
                .zip(wt_out.par_chunks_mut(width))
                .zip(var_out.par_chunks_mut(width))
                .enumerate()
                .for_each(|(row_in_chunk, ((cov_row, wt_row), var_row))| {
                    let row_base = row_in_chunk * width;
                    for px in 0..width {
                        let local = row_base + px;
                        let mut count = 0u32;
                        let mut wsum = 0.0f32;
                        let mut wsq = 0.0f32;
                        for (frame_idx, cov) in cov_chunks.iter().enumerate() {
                            let c = cov.map_or(1.0, |map| map[local]);
                            if c > COVERAGE_EPSILON {
                                let w = frame_weight(frame_idx) * c;
                                wsum += w;
                                wsq += w * w;
                                count += 1;
                            }
                        }
                        cov_row[px] = count as f32 * inv_frames;
                        wt_row[px] = wsum;
                        var_row[px] = if wsum > 0.0 { wsq / (wsum * wsum) } else { 0.0 };
                    }
                });

            start_row = end_row;
        }

        StackProduct {
            image,
            coverage,
            weight,
            variance,
        }
    }

    /// Coverage-weighted combine: a frame contributes at a pixel only where its coverage exceeds
    /// [`COVERAGE_EPSILON`], weighted by `coverage × per-frame weight`. Excluding sub-ε coverage
    /// keeps warp border-fill out of the rejection set, so warped-frame borders don't drag the
    /// stacked edges dark; a pixel no frame covers gets `0`.
    pub fn process_chunked_weighted<Combine>(
        &self,
        weights: Option<&[f32]>,
        frame_norms: Option<&[FrameNorm]>,
        combine: Combine,
    ) -> PixelData
    where
        Combine: Fn(&mut [f32], Option<&[f32]>, &mut ScratchBuffers) -> f32 + Sync,
    {
        if let Some(w) = weights {
            assert_eq!(
                w.len(),
                self.frames.len(),
                "Weight count must match frame count"
            );
        }
        // An in-memory stack is one chunk, so the per-chunk cancel check in
        // `process_chunks` can't interrupt the combine — poll per row here too.
        let cancel = self.core.cancel.clone();
        self.core.process_chunks(&self.frames, |output_slice, ctx| {
            let ChunkContext {
                frames,
                width,
                channel,
                pixel_offset,
            } = ctx;
            let frame_count = frames.len();
            let chunk_pixels = output_slice.len();
            // Per-frame coverage slice for this chunk's rows; `None` = fully covered.
            let coverage: Vec<Option<&[f32]>> = self
                .frames
                .iter()
                .map(|frame| {
                    frame
                        .coverage
                        .as_ref()
                        .map(|plane| plane.chunk(pixel_offset, pixel_offset + chunk_pixels))
                })
                .collect();
            output_slice
                .par_chunks_mut(width)
                .enumerate()
                .for_each_init(
                    || {
                        (
                            vec![0.0f32; frame_count],
                            vec![0.0f32; frame_count],
                            ScratchBuffers::new(frame_count),
                        )
                    },
                    |(values, eff_weights, scratch), (row_in_chunk, row_output)| {
                        // Cancelled: skip the row's work (output stays zero; the
                        // caller discards the partial result and reports Cancelled).
                        if cancel.is_cancelled() {
                            return;
                        }
                        let row_offset = row_in_chunk * width;
                        for (pixel_in_row, out) in row_output.iter_mut().enumerate() {
                            let pixel_idx = row_offset + pixel_in_row;
                            let mut covered = 0usize;
                            for (frame_idx, chunk) in frames.iter().enumerate() {
                                let c = match coverage[frame_idx] {
                                    Some(map) => map[pixel_idx],
                                    None => 1.0,
                                };
                                if c > COVERAGE_EPSILON {
                                    let v = match frame_norms {
                                        Some(fnm) => {
                                            let cn = fnm[frame_idx].channels[channel];
                                            chunk[pixel_idx] * cn.gain + cn.offset
                                        }
                                        None => chunk[pixel_idx],
                                    };
                                    values[covered] = v;
                                    eff_weights[covered] =
                                        weights.map_or(1.0, |w| w[frame_idx]) * c;
                                    covered += 1;
                                }
                            }
                            *out = if covered == 0 {
                                0.0
                            } else {
                                debug_assert!(
                                    values[..covered].iter().all(|v| v.is_finite()),
                                    "non-finite pixel value entered the combine",
                                );
                                combine(
                                    &mut values[..covered],
                                    Some(&eff_weights[..covered]),
                                    scratch,
                                )
                            };
                        }
                    },
                );
        })
    }
}

/// Generate cache filename for a specific channel.
fn channel_cache_filename(base_filename: &str, channel: usize) -> String {
    let stem = base_filename.strip_suffix(".bin").unwrap_or(base_filename);
    format!("{stem}_c{channel}.bin")
}

/// Try to reuse an existing channel cache file if it exists and has matching size.
///
/// Returns true if the file can be reused, false if it needs to be rewritten.
fn try_reuse_channel_cache_file(path: &Path, expected_dims: ImageDimensions) -> bool {
    let Ok(metadata) = std::fs::metadata(path) else {
        return false;
    };

    let pixels_per_channel = expected_dims.size.x * expected_dims.size.y;
    let expected_size = (pixels_per_channel * size_of::<f32>()) as u64;
    if metadata.len() != expected_size {
        return false;
    }

    tracing::debug!("Reusing existing channel cache file: {:?}", path);
    true
}

/// Get the source file's modification time as seconds since epoch.
fn source_mtime(path: &Path) -> Option<u64> {
    let metadata = std::fs::metadata(path).ok()?;
    let modified = metadata.modified().ok()?;
    Some(
        modified
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs(),
    )
}

/// Path for the sidecar metadata file that stores source mtime.
fn meta_path(cache_dir: &Path, base_filename: &str) -> PathBuf {
    cache_dir.join(format!("{}.meta", base_filename.trim_end_matches(".bin")))
}

/// Write source mtime to sidecar file.
fn write_source_meta(cache_dir: &Path, base_filename: &str, mtime: u64) {
    let path = meta_path(cache_dir, base_filename);
    let _ = std::fs::write(&path, mtime.to_le_bytes());
}

/// Check if cached data is still valid by comparing source mtime.
/// Returns true if the sidecar exists and its stored mtime matches the source.
fn validate_source_meta(cache_dir: &Path, base_filename: &str, source: &Path) -> bool {
    let current_mtime = match source_mtime(source) {
        Some(m) => m,
        None => return false,
    };
    let path = meta_path(cache_dir, base_filename);
    let Ok(bytes) = std::fs::read(&path) else {
        return false;
    };
    if bytes.len() != 8 {
        return false;
    }
    let stored_mtime = u64::from_le_bytes(bytes.try_into().unwrap());
    stored_mtime == current_mtime
}

/// Write a single channel to a binary cache file.
fn write_channel_cache_file(path: &Path, channel_data: &[f32]) -> Result<(), Error> {
    let file = File::create(path).map_err(|e| Error::CreateCacheFile {
        path: path.to_path_buf(),
        source: e,
    })?;
    let mut writer = BufWriter::new(file);
    let map_write_err = |e| Error::WriteCacheFile {
        path: path.to_path_buf(),
        source: e,
    };

    let bytes: &[u8] = bytemuck::cast_slice(channel_data);
    writer.write_all(bytes).map_err(&map_write_err)?;
    writer.flush().map_err(map_write_err)?;

    Ok(())
}

/// Open and memory-map a channel cache file.
/// Advises the OS for sequential access (the stacking pipeline reads row-by-row).
fn mmap_channel_file(channel_path: PathBuf) -> Result<Mmap, Error> {
    let file = File::open(&channel_path).map_err(|e| Error::OpenCacheFile {
        path: channel_path.clone(),
        source: e,
    })?;
    let mmap = unsafe {
        Mmap::map(&file).map_err(|e| Error::MmapCacheFile {
            path: channel_path,
            source: e,
        })?
    };

    #[cfg(unix)]
    {
        use memmap2::Advice;
        let _ = mmap.advise(Advice::Sequential);
    }

    Ok(mmap)
}

/// Load an image and cache it, or reuse existing cache files if valid.
/// Returns the [`Frame`] with memory-mapped channel planes.
fn load_and_cache_frame<I: StackableImage>(
    cache_dir: &Path,
    base_filename: &str,
    source_path: &Path,
    dimensions: ImageDimensions,
    frame_index: usize,
) -> Result<(Frame, FrameStats), Error> {
    let channels = dimensions.channels;

    // Check if all channel files exist, have correct size, and source hasn't changed
    let meta_valid = validate_source_meta(cache_dir, base_filename, source_path);
    let has_stats = stats_path(cache_dir, base_filename).exists();
    let can_reuse = meta_valid
        && has_stats
        && (0..channels).all(|c| {
            let channel_path = cache_dir.join(channel_cache_filename(base_filename, c));
            try_reuse_channel_cache_file(&channel_path, dimensions)
        });

    if can_reuse {
        // Reuse existing cache files - just mmap them
        let mut planes = ArrayVec::new();
        for c in 0..channels {
            let channel_path = cache_dir.join(channel_cache_filename(base_filename, c));
            planes.push(Plane::Mapped(mmap_channel_file(channel_path)?));
        }
        tracing::debug!(
            source = %source_path.display(),
            "Reusing existing cache files"
        );
        let frame = Frame { channels: planes };
        let stats = read_frame_stats(cache_dir, base_filename)
            .expect("stats sidecar missing for valid cache");
        Ok((frame, stats))
    } else {
        // Load image and write to cache
        let image = I::load(source_path)?;

        if image.dimensions() != dimensions {
            return Err(Error::DimensionMismatch {
                index: frame_index,
                expected: dimensions,
                actual: image.dimensions(),
            });
        }

        let stats = compute_frame_stats(&image);
        let result = cache_image_channels(cache_dir, base_filename, &image, dimensions)?;

        // Record source mtime and stats so future runs skip recomputation
        let mtime = source_mtime(source_path).expect("source file must have mtime");
        write_source_meta(cache_dir, base_filename, mtime);
        write_frame_stats(cache_dir, base_filename, &stats);

        Ok((result, stats))
    }
}

/// Path for the sidecar stats file.
fn stats_path(cache_dir: &Path, base_filename: &str) -> PathBuf {
    cache_dir.join(format!("{}.stats", base_filename.trim_end_matches(".bin")))
}

/// Write frame stats to a sidecar file.
/// Format: [n_channels: u8] [median_0: f32] [mad_0: f32] [median_1: f32] ...
fn write_frame_stats(cache_dir: &Path, base_filename: &str, stats: &FrameStats) {
    let path = stats_path(cache_dir, base_filename);
    let n = stats.channels.len();
    let mut buf = Vec::with_capacity(1 + n * 8);
    buf.push(n as u8);
    for ch in &stats.channels {
        buf.extend_from_slice(&ch.median.to_le_bytes());
        buf.extend_from_slice(&ch.mad.to_le_bytes());
    }
    let _ = std::fs::write(&path, buf);
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

/// Cache all channels of an image to separate files and return a memory-mapped [`Frame`].
fn cache_image_channels(
    cache_dir: &Path,
    base_filename: &str,
    image: &impl StackableImage,
    dimensions: ImageDimensions,
) -> Result<Frame, Error> {
    Ok(Frame {
        channels: spill_channels(cache_dir, base_filename, image, dimensions)?,
    })
}

/// Spill an image's channels to `cache_dir/<base>_c<ch>.bin` and return them as memory-mapped
/// [`Plane`]s. The tiering primitive shared by the path-based loader, the streaming calibrate spill,
/// and the streaming warp spill.
pub(crate) fn spill_channels(
    cache_dir: &Path,
    base_filename: &str,
    image: &impl StackableImage,
    dimensions: ImageDimensions,
) -> Result<ArrayVec<Plane, 3>, Error> {
    let mut planes = ArrayVec::new();
    for c in 0..dimensions.channels {
        let channel_path = cache_dir.join(channel_cache_filename(base_filename, c));
        if !try_reuse_channel_cache_file(&channel_path, dimensions) {
            write_channel_cache_file(&channel_path, image.channel(c))?;
        }
        planes.push(Plane::Mapped(mmap_channel_file(channel_path)?));
    }
    Ok(planes)
}

/// Spill a warped light frame (channels + optional coverage) to the disk cache and return the
/// memory-mapped [`WeightedFrame`] plus its [`FrameStats`] — computed from the in-RAM `image` before
/// it is dropped, so the streaming pipeline never re-reads the planes just for stats.
pub(crate) fn spill_weighted_frame(
    cache_dir: &Path,
    base_filename: &str,
    image: AstroImage,
    coverage: Option<Buffer2<f32>>,
    dimensions: ImageDimensions,
) -> Result<(WeightedFrame, FrameStats), Error> {
    let stats = compute_frame_stats(&image);
    let channels = spill_channels(cache_dir, base_filename, &image, dimensions)?;
    let coverage = match coverage {
        Some(cov) => {
            let path = cache_dir.join(format!("{base_filename}_coverage.bin"));
            write_channel_cache_file(&path, &cov)?;
            Some(Plane::Mapped(mmap_channel_file(path)?))
        }
        None => None,
    };
    Ok((WeightedFrame { channels, coverage }, stats))
}

/// Reconstruct an `AstroImage` from spilled (memory-mapped) channels — used by the streaming warp to
/// pull a calibrated frame back into RAM for [`crate::stacking::registration::warp`], which needs a
/// full `AstroImage`. Copies the mmap'd planes into owned buffers (bounded by warp concurrency).
pub(crate) fn image_from_spilled_channels(
    channels: &[Plane],
    dimensions: ImageDimensions,
) -> AstroImage {
    let n = dimensions.size.x * dimensions.size.y;
    let planes = channels.iter().map(|p| p.chunk(0, n).to_vec());
    // Metadata stays at the default: this image is only warped then re-spilled, and the spill keeps
    // channels only — the stacked master's metadata is supplied separately to the combine.
    AstroImage::from_planar_channels(dimensions, planes)
}

/// Delete a calibrated frame's spilled channel files once its warp has consumed it (keeps peak temp
/// usage at ~`N × warped-frame` rather than `N × (calibrated + warped)`).
pub(crate) fn remove_spilled_channels(cache_dir: &Path, base_filename: &str, channels: usize) {
    for c in 0..channels {
        let _ = std::fs::remove_file(cache_dir.join(channel_cache_filename(base_filename, c)));
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::io::astro_image::AstroImage;
    use crate::io::astro_image::cfa::CfaType;

    /// Create an in-memory [`LightCache`] from loaded images, with no coverage (test helper).
    pub(crate) fn make_test_cache(images: Vec<AstroImage>) -> LightCache {
        let frames = images.into_iter().map(StackFrame::from).collect();
        LightCache::from_stack_frames(frames, &CacheConfig::default(), ProgressCallback::default())
            .expect("test images must be non-empty and dimension-consistent")
    }

    #[test]
    fn finish_product_uniform_equal_weights() {
        // 4 frames, no coverage maps → fast path. Equal weights: every pixel sees all 4 frames at
        // weight 1, so weight = Σw = 4, variance = Σw²/(Σw)² = 4/16 = 0.25, coverage = 4/4 = 1.
        let dims = ImageDimensions::new((3, 2), 1);
        let images: Vec<AstroImage> = (0..4)
            .map(|i| AstroImage::from_pixels(dims, vec![i as f32; 6]))
            .collect();
        let product = make_test_cache(images)
            .finish_product(AstroImage::from_pixels(dims, vec![0.25; 6]), None);
        assert_eq!(product.image.channel(0).pixels(), &[0.25; 6]);
        for p in 0..6 {
            assert_eq!(product.coverage[p], 1.0);
            assert_eq!(product.weight[p], 4.0);
            assert_eq!(product.variance[p], 0.25);
        }
    }

    #[test]
    fn finish_product_uniform_manual_weights() {
        // weights [1,2,3,4], full coverage: weight = 10, Σw² = 1+4+9+16 = 30, variance = 30/100 = 0.30.
        let dims = ImageDimensions::new((2, 1), 1);
        let images: Vec<AstroImage> = (0..4)
            .map(|_| AstroImage::from_pixels(dims, vec![0.5; 2]))
            .collect();
        let product = make_test_cache(images).finish_product(
            AstroImage::from_pixels(dims, vec![0.25; 2]),
            Some(&[1.0, 2.0, 3.0, 4.0]),
        );
        for p in 0..2 {
            assert_eq!(product.coverage[p], 1.0);
            assert_eq!(product.weight[p], 10.0);
            assert!(
                (product.variance[p] - 0.30).abs() < 1e-6,
                "variance = {}",
                product.variance[p]
            );
        }
    }

    #[test]
    fn finish_product_partial_coverage() {
        // width-2 frames; px0 fully covered by all 4, px1 covered by f0(1.0), f1(0.5), f3(1.0) and
        // dropped by f2 (coverage 0 < ε). Equal weights. Exercises the per-pixel (non-fast) path.
        //   px0: count 4, Σwc = 4,   Σ(wc)² = 4    → coverage 1.0,  weight 4.0, variance 4/16  = 0.25
        //   px1: count 3, Σwc = 2.5, Σ(wc)² = 2.25 → coverage 0.75, weight 2.5, variance 2.25/6.25 = 0.36
        let dims = ImageDimensions::new((2, 1), 1);
        let cov = [[1.0_f32, 1.0], [1.0, 0.5], [1.0, 0.0], [1.0, 1.0]];
        let frames: Vec<StackFrame> = cov
            .iter()
            .map(|c| StackFrame {
                image: AstroImage::from_pixels(dims, vec![0.5, 0.5]),
                coverage: Some(Buffer2::new(2, 1, c.to_vec())),
            })
            .collect();
        let cache = LightCache::from_stack_frames(
            frames,
            &CacheConfig::default(),
            ProgressCallback::default(),
        )
        .expect("frames are valid");
        let product = cache.finish_product(AstroImage::from_pixels(dims, vec![0.25, 0.25]), None);

        assert_eq!(product.coverage[0], 1.0);
        assert_eq!(product.weight[0], 4.0);
        assert_eq!(product.variance[0], 0.25);

        assert_eq!(product.coverage[1], 0.75);
        assert_eq!(product.weight[1], 2.5);
        assert!(
            (product.variance[1] - 0.36).abs() < 1e-6,
            "variance = {}",
            product.variance[1]
        );
    }

    /// Build an in-memory [`CfaCache`] from single-channel CFA frame pixels (test helper for the
    /// plain combine; `process_chunked` ignores stats, so `channel_stats` is left empty).
    fn make_cfa_cache(frames_pixels: Vec<Vec<f32>>, dims: ImageDimensions) -> CfaCache {
        let frames = frames_pixels
            .into_iter()
            .map(|pixels| {
                let image = CfaImage {
                    data: Buffer2::new(dims.size.x, dims.size.y, pixels),
                    metadata: AstroImageMetadata {
                        cfa_type: Some(CfaType::Mono),
                        ..Default::default()
                    },
                };
                image_to_frame(image)
            })
            .collect();
        CfaCache {
            frames,
            core: CacheCore {
                cache_dir: None,
                dimensions: dims,
                metadata: AstroImageMetadata::default(),
                channel_stats: vec![],
                config: CacheConfig::default(),
                progress: ProgressCallback::default(),
                cancel: CancelToken::never(),
            },
        }
    }

    // ========== Storage Type Selection Tests ==========

    // ========== Cache File Tests ==========

    #[test]
    fn test_channel_cache_roundtrip() {
        let temp_dir = std::env::temp_dir().join("lumos_channel_cache_test");
        std::fs::create_dir_all(&temp_dir).unwrap();

        let dims = ImageDimensions::new((4, 3), 3);
        let pixels: Vec<f32> = (0..36).map(|i| i as f32).collect();
        let image = AstroImage::from_pixels(dims, pixels);

        let base_filename = "test_frame.bin";
        let cached_frame = cache_image_channels(&temp_dir, base_filename, &image, dims).unwrap();

        // Should create 3 channel files
        assert_eq!(cached_frame.channels.len(), 3);

        // Verify each channel plane contains the correct data
        for (c, cached_channel) in cached_frame.channels.iter().enumerate() {
            let read_channel = cached_channel.chunk(0, dims.size.x * dims.size.y);
            let expected_channel = image.channel(c).pixels();
            assert_eq!(read_channel, expected_channel);
        }

        // Cleanup
        drop(cached_frame);
        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_try_reuse_channel_cache_file() {
        let temp_dir = std::env::temp_dir().join("lumos_reuse_test");
        std::fs::create_dir_all(&temp_dir).unwrap();

        let dims = ImageDimensions::new((4, 3), 1);
        let pixels: Vec<f32> = (0..12).map(|i| i as f32).collect();
        let image = AstroImage::from_pixels(dims, pixels);

        let cache_path = temp_dir.join("reuse_c0.bin");
        write_channel_cache_file(&cache_path, image.channel(0)).unwrap();

        // Matching dimensions: reusable
        assert!(try_reuse_channel_cache_file(&cache_path, dims));

        // Different dimensions: not reusable
        assert!(!try_reuse_channel_cache_file(
            &cache_path,
            ImageDimensions::new((8, 3), 1)
        ));

        // Nonexistent file: not reusable
        assert!(!try_reuse_channel_cache_file(
            Path::new("/nonexistent/file.bin"),
            dims
        ));

        // Wrong size file: not reusable
        let wrong_size_path = temp_dir.join("wrong_size.bin");
        std::fs::write(&wrong_size_path, b"too short").unwrap();
        assert!(!try_reuse_channel_cache_file(&wrong_size_path, dims));

        // Cleanup
        let _ = std::fs::remove_file(&cache_path);
        let _ = std::fs::remove_file(&wrong_size_path);
        let _ = std::fs::remove_dir(&temp_dir);
    }

    #[test]
    fn test_channel_cache_filename() {
        assert_eq!(channel_cache_filename("abc123.bin", 0), "abc123_c0.bin");
        assert_eq!(channel_cache_filename("abc123.bin", 1), "abc123_c1.bin");
        assert_eq!(channel_cache_filename("abc123.bin", 2), "abc123_c2.bin");
    }

    // ========== Error Path Tests ==========

    #[test]
    fn test_from_paths_errors() {
        let config = CacheConfig::default();

        // Empty paths
        let result = LightCache::from_paths(
            &Vec::<PathBuf>::new(),
            &config,
            ProgressCallback::default(),
            CancelToken::never(),
        );
        assert!(matches!(result.unwrap_err(), Error::NoFrames));

        // Nonexistent file
        let result = LightCache::from_paths(
            &[PathBuf::from("/nonexistent/path/image.fits")],
            &config,
            ProgressCallback::default(),
            CancelToken::never(),
        );
        assert!(matches!(result.unwrap_err(), Error::ImageLoad { .. }));
    }

    // ========== Processing Tests ==========

    #[test]
    fn test_process_chunked_median() {
        // Create in-memory cache with 3 grayscale frames
        let dims = ImageDimensions::new((4, 4), 1);
        let images = vec![
            AstroImage::from_pixels(dims, vec![1.0; 16]),
            AstroImage::from_pixels(dims, vec![3.0; 16]),
            AstroImage::from_pixels(dims, vec![2.0; 16]),
        ];

        let cache = make_test_cache(images);

        // Median of [1, 3, 2] = 2
        let result = cache.process_chunked_weighted(None, None, |values, _, _| {
            values.sort_by(|a, b| a.partial_cmp(b).unwrap());
            values[values.len() / 2]
        });

        assert_eq!(result.channels(), 1);
        assert_eq!(result.channel(0).len(), 16);
        for &pixel in result.channel(0).pixels() {
            assert!((pixel - 2.0).abs() < f32::EPSILON);
        }
    }

    #[test]
    fn test_process_chunked_rgb() {
        // Create in-memory cache with 2 RGB frames
        let dims = ImageDimensions::new((2, 2), 3);
        // Frame 1: R=1, G=2, B=3 for all pixels
        let pixels1: Vec<f32> = (0..4).flat_map(|_| vec![1.0, 2.0, 3.0]).collect();
        // Frame 2: R=5, G=6, B=7 for all pixels
        let pixels2: Vec<f32> = (0..4).flat_map(|_| vec![5.0, 6.0, 7.0]).collect();

        let images = vec![
            AstroImage::from_pixels(dims, pixels1),
            AstroImage::from_pixels(dims, pixels2),
        ];

        let cache = make_test_cache(images);

        // Mean: R=(1+5)/2=3, G=(2+6)/2=4, B=(3+7)/2=5
        let result = cache.process_chunked_weighted(None, None, |values, _, _| {
            values.iter().sum::<f32>() / values.len() as f32
        });

        assert_eq!(result.channels(), 3);
        for &pixel in result.channel(0).pixels() {
            assert!((pixel - 3.0).abs() < f32::EPSILON, "R channel");
        }
        for &pixel in result.channel(1).pixels() {
            assert!((pixel - 4.0).abs() < f32::EPSILON, "G channel");
        }
        for &pixel in result.channel(2).pixels() {
            assert!((pixel - 5.0).abs() < f32::EPSILON, "B channel");
        }
    }

    #[test]
    fn test_process_chunked_with_weights() {
        let dims = ImageDimensions::new((2, 2), 1);
        let images = vec![
            AstroImage::from_pixels(dims, vec![10.0; 4]),
            AstroImage::from_pixels(dims, vec![20.0; 4]),
        ];

        let cache = make_test_cache(images);

        // Weighted mean with weights [1, 3]: (10*1 + 20*3) / (1+3) = 70/4 = 17.5
        let weights = vec![1.0, 3.0];
        let result = cache.process_chunked_weighted(Some(&weights), None, |values, w, _| {
            let w = w.unwrap();
            let sum: f32 = values.iter().zip(w.iter()).map(|(v, wt)| v * wt).sum();
            let weight_sum: f32 = w.iter().sum();
            sum / weight_sum
        });

        for &pixel in result.channel(0).pixels() {
            assert!((pixel - 17.5).abs() < f32::EPSILON);
        }
    }

    #[test]
    fn test_cfa_cache_plain_combine() {
        // The plain `CfaCache::process_chunked` path (calibration): no coverage, every frame
        // contributes at every pixel.
        let dims = ImageDimensions::new((2, 2), 1);

        // Median of [1, 3, 2] = 2 at every pixel.
        let cache = make_cfa_cache(vec![vec![1.0; 4], vec![3.0; 4], vec![2.0; 4]], dims);
        let median = cache.process_chunked(None, None, |values, _, _| {
            values.sort_by(|a, b| a.partial_cmp(b).unwrap());
            values[values.len() / 2]
        });
        assert_eq!(median.channels(), 1);
        for &pixel in median.channel(0).pixels() {
            assert!(
                (pixel - 2.0).abs() < f32::EPSILON,
                "CFA plain median should be 2, got {pixel}"
            );
        }

        // Weighted mean of [10, 20] with weights [1, 3] = (10 + 60) / 4 = 17.5 — weights flow
        // through to the combine closure unchanged (no coverage scaling on the plain path).
        let cache = make_cfa_cache(vec![vec![10.0; 4], vec![20.0; 4]], dims);
        let weights = [1.0, 3.0];
        let weighted = cache.process_chunked(Some(&weights), None, |values, w, _| {
            let w = w.unwrap();
            let sum: f32 = values.iter().zip(w).map(|(v, wt)| v * wt).sum();
            sum / w.iter().sum::<f32>()
        });
        for &pixel in weighted.channel(0).pixels() {
            assert!(
                (pixel - 17.5).abs() < f32::EPSILON,
                "CFA plain weighted mean should be 17.5, got {pixel}"
            );
        }
    }

    #[test]
    fn test_frame_count() {
        let dims = ImageDimensions::new((2, 2), 1);
        let images = vec![
            AstroImage::from_pixels(dims, vec![1.0; 4]),
            AstroImage::from_pixels(dims, vec![2.0; 4]),
            AstroImage::from_pixels(dims, vec![3.0; 4]),
        ];

        let cache = make_test_cache(images);

        assert_eq!(cache.frames.len(), 3);
    }

    #[test]
    fn test_cleanup_removes_files() {
        let temp_dir = std::env::temp_dir().join("lumos_cleanup_test");
        std::fs::create_dir_all(&temp_dir).unwrap();

        // Create a real cached frame using cache_image_channels
        let dims = ImageDimensions::new((2, 2), 3);
        let pixels: Vec<f32> = (0..12).map(|i| i as f32).collect();
        let image = AstroImage::from_pixels(dims, pixels);

        let cached_frame =
            cache_image_channels(&temp_dir, "cleanup_test.bin", &image, dims).unwrap();

        // Verify cache dir has files
        assert!(temp_dir.exists());
        assert!(temp_dir.read_dir().unwrap().count() > 0);

        // Use keep_cache: false to actually test cleanup
        let config = CacheConfig {
            keep_cache: false,
            ..Default::default()
        };

        let cache = CfaCache {
            frames: vec![cached_frame],
            core: CacheCore {
                cache_dir: Some(temp_dir.clone()),
                dimensions: dims,
                metadata: AstroImageMetadata::default(),
                channel_stats: vec![],
                config,
                progress: ProgressCallback::default(),
                cancel: CancelToken::never(),
            },
        };

        // Drop the cache - should trigger cleanup via the core's Drop
        drop(cache);

        // Entire cache directory should be removed
        assert!(
            !temp_dir.exists(),
            "Cache directory should be deleted on cleanup"
        );
    }

    #[test]
    fn test_read_channel_chunk_in_memory() {
        let dims = ImageDimensions::new((4, 3), 1);
        // Pixels 0-11 in row-major order
        let pixels: Vec<f32> = (0..12).map(|i| i as f32).collect();
        let images = vec![AstroImage::from_pixels(dims, pixels)];

        let cache = make_test_cache(images);

        // Read row 1 (pixels 4-7)
        let chunk = cache.core.read_channel_chunk(&cache.frames, 0, 0, 1, 2);
        let expected: Vec<f32> = (4..8).map(|i| i as f32).collect();
        assert_eq!(chunk, &expected[..]);

        // Read all rows
        let all = cache.core.read_channel_chunk(&cache.frames, 0, 0, 0, 3);
        assert_eq!(all.len(), 12);
    }

    #[test]
    fn test_read_channel_chunk_disk_backed() {
        let temp_dir = std::env::temp_dir().join("lumos_read_chunk_disk_test");
        std::fs::create_dir_all(&temp_dir).unwrap();

        let dims = ImageDimensions::new((4, 3), 1);
        let pixels: Vec<f32> = (0..12).map(|i| i as f32).collect();
        let image = AstroImage::from_pixels(dims, pixels);

        // Cache the image to disk
        let base_filename = "test_chunk.bin";
        let cached_frame = cache_image_channels(&temp_dir, base_filename, &image, dims).unwrap();

        let cache = CfaCache {
            frames: vec![cached_frame],
            core: CacheCore {
                cache_dir: Some(temp_dir.clone()),
                dimensions: dims,
                metadata: AstroImageMetadata::default(),
                channel_stats: vec![],
                config: CacheConfig::default(),
                progress: ProgressCallback::default(),
                cancel: CancelToken::never(),
            },
        };

        // Read row 1 (pixels 4-7)
        let chunk = cache.core.read_channel_chunk(&cache.frames, 0, 0, 1, 2);
        let expected: Vec<f32> = (4..8).map(|i| i as f32).collect();
        assert_eq!(chunk, &expected[..]);

        // Read all rows
        let all = cache.core.read_channel_chunk(&cache.frames, 0, 0, 0, 3);
        assert_eq!(all.len(), 12);
        for (i, &val) in all.iter().enumerate() {
            assert!((val - i as f32).abs() < f32::EPSILON);
        }

        // Cleanup
        cache.core.cleanup();
    }

    #[test]
    fn test_frame_count_disk_backed() {
        let temp_dir = std::env::temp_dir().join("lumos_frame_count_disk_test");
        std::fs::create_dir_all(&temp_dir).unwrap();

        let dims = ImageDimensions::new((2, 2), 1);

        // Create 3 cached frames
        let mut frames = Vec::new();
        for i in 0..3 {
            let pixels: Vec<f32> = vec![i as f32; 4];
            let image = AstroImage::from_pixels(dims, pixels);
            let base_filename = format!("frame{}.bin", i);
            let cached_frame =
                cache_image_channels(&temp_dir, &base_filename, &image, dims).unwrap();
            frames.push(cached_frame);
        }

        let cache = CfaCache {
            frames,
            core: CacheCore {
                cache_dir: Some(temp_dir.clone()),
                dimensions: dims,
                metadata: AstroImageMetadata::default(),
                channel_stats: vec![],
                config: CacheConfig::default(),
                progress: ProgressCallback::default(),
                cancel: CancelToken::never(),
            },
        };

        assert_eq!(cache.frames.len(), 3);

        // Cleanup
        cache.core.cleanup();
    }

    #[test]
    fn test_load_and_cache_frame_fresh() {
        let temp_dir = std::env::temp_dir().join("lumos_load_cache_fresh_test");
        std::fs::create_dir_all(&temp_dir).unwrap();

        let dims = ImageDimensions::new((4, 3), 1);
        let pixels: Vec<f32> = (0..12).map(|i| i as f32).collect();
        let image = AstroImage::from_pixels(dims, pixels.clone());

        // Write a temp TIFF file to load from
        let source_path = temp_dir.join("source.tiff");
        image.save(&source_path).unwrap();

        let base_filename = "cached_frame.bin";

        // First call should load and cache
        let cached_frame =
            load_and_cache_frame::<AstroImage>(&temp_dir, base_filename, &source_path, dims, 0)
                .unwrap();

        let (cached_frame, _stats) = cached_frame;
        assert_eq!(cached_frame.channels.len(), 1);

        // Verify cached data matches original
        let cached_data = cached_frame.channels[0].chunk(0, dims.size.x * dims.size.y);
        assert_eq!(cached_data, &pixels[..]);

        // Cleanup
        drop(cached_frame);
        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_load_and_cache_frame_reuse() {
        let temp_dir = std::env::temp_dir().join("lumos_load_cache_reuse_test");
        std::fs::create_dir_all(&temp_dir).unwrap();

        let dims = ImageDimensions::new((4, 3), 1);
        let pixels: Vec<f32> = (0..12).map(|i| i as f32).collect();
        let image = AstroImage::from_pixels(dims, pixels.clone());

        // Write a temp TIFF file
        let source_path = temp_dir.join("source.tiff");
        image.save(&source_path).unwrap();

        let base_filename = "cached_frame.bin";

        // First call - creates cache
        let first_frame =
            load_and_cache_frame::<AstroImage>(&temp_dir, base_filename, &source_path, dims, 0)
                .unwrap();

        // Second call - should reuse cache
        let second_frame =
            load_and_cache_frame::<AstroImage>(&temp_dir, base_filename, &source_path, dims, 0)
                .unwrap();

        // Both should have same data
        let n = dims.size.x * dims.size.y;
        let first_data = first_frame.0.channels[0].chunk(0, n);
        let second_data = second_frame.0.channels[0].chunk(0, n);
        assert_eq!(first_data, second_data);
        assert_eq!(first_data, &pixels[..]);

        // Cleanup
        drop(first_frame);
        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_load_and_cache_frame_dimension_mismatch() {
        let temp_dir = std::env::temp_dir().join("lumos_load_cache_mismatch_test");
        std::fs::create_dir_all(&temp_dir).unwrap();

        // Create image with different dimensions than expected
        let actual_dims = ImageDimensions::new((4, 3), 1);
        let pixels: Vec<f32> = (0..12).map(|i| i as f32).collect();
        let image = AstroImage::from_pixels(actual_dims, pixels);

        let source_path = temp_dir.join("source.tiff");
        image.save(&source_path).unwrap();

        // Try to load with wrong expected dimensions
        let expected_dims = ImageDimensions::new((8, 6), 1);
        let result = load_and_cache_frame::<AstroImage>(
            &temp_dir,
            "cached.bin",
            &source_path,
            expected_dims,
            5,
        );

        assert!(matches!(
            result.unwrap_err(),
            Error::DimensionMismatch {
                index: 5,
                expected,
                actual,
                ..
            } if expected == expected_dims && actual == actual_dims
        ));

        // Cleanup
        let _ = std::fs::remove_file(&source_path);
        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_compute_channel_stats_grayscale() {
        // 3 grayscale frames, 3x3 pixels each
        let dims = ImageDimensions::new((3, 3), 1);

        // Frame 0: all 5.0 → median=5.0, MAD=0.0
        let frame0 = AstroImage::from_pixels(dims, vec![5.0; 9]);

        // Frame 1: [1,2,3,4,5,6,7,8,9] → median=5.0, deviations=[4,3,2,1,0,1,2,3,4] → MAD=2.0
        let frame1 = AstroImage::from_pixels(dims, (1..=9).map(|i| i as f32).collect());

        // Frame 2: [10,10,10,20,20,20,30,30,30] → median=20.0, deviations=[10,10,10,0,0,0,10,10,10] → MAD=10.0
        let frame2 = AstroImage::from_pixels(
            dims,
            vec![10.0, 10.0, 10.0, 20.0, 20.0, 20.0, 30.0, 30.0, 30.0],
        );

        let cache = make_test_cache(vec![frame0, frame1, frame2]);
        let stats = &cache.core.channel_stats;

        assert_eq!(stats.len(), 3); // 3 frames
        assert_eq!(stats[0].channels.len(), 1);
        assert!((stats[0].channels[0].median - 5.0).abs() < f32::EPSILON);
        assert!((stats[0].channels[0].mad - 0.0).abs() < f32::EPSILON);
        assert!((stats[1].channels[0].median - 5.0).abs() < f32::EPSILON);
        assert!((stats[1].channels[0].mad - 2.0).abs() < f32::EPSILON);
        assert!((stats[2].channels[0].median - 20.0).abs() < f32::EPSILON);
        assert!((stats[2].channels[0].mad - 10.0).abs() < f32::EPSILON);
    }

    #[test]
    fn test_compute_channel_stats_rgb() {
        // 2 RGB frames, 2x2 pixels each
        let dims = ImageDimensions::new((2, 2), 3);

        // Frame 0: R=[1,3,5,7] G=[10,10,10,10] B=[0,0,100,100]
        let frame0 = AstroImage::from_planar_channels(
            dims,
            vec![
                vec![1.0, 3.0, 5.0, 7.0],
                vec![10.0, 10.0, 10.0, 10.0],
                vec![0.0, 0.0, 100.0, 100.0],
            ],
        );
        // Frame 0 expected:
        //   R: median=4.0 (avg of 3,5), deviations=[3,1,1,3] → MAD=2.0 (avg of 1,3)
        //   G: median=10.0, MAD=0.0
        //   B: median=50.0 (avg of 0,100), deviations=[50,50,50,50] → MAD=50.0

        // Frame 1: R=[2,2,2,2] G=[1,2,3,4] B=[10,20,30,40]
        let frame1 = AstroImage::from_planar_channels(
            dims,
            vec![
                vec![2.0, 2.0, 2.0, 2.0],
                vec![1.0, 2.0, 3.0, 4.0],
                vec![10.0, 20.0, 30.0, 40.0],
            ],
        );
        // Frame 1 expected:
        //   R: median=2.0, MAD=0.0
        //   G: median=2.5, deviations=[1.5,0.5,0.5,1.5] → MAD=1.0
        //   B: median=25.0, deviations=[15,5,5,15] → MAD=10.0

        let cache = make_test_cache(vec![frame0, frame1]);
        let stats = &cache.core.channel_stats;

        assert_eq!(stats.len(), 2); // 2 frames
        assert_eq!(stats[0].channels.len(), 3); // 3 channels each

        // Frame 0
        assert!(
            (stats[0].channels[0].median - 4.0).abs() < f32::EPSILON,
            "F0 R median"
        );
        assert!(
            (stats[0].channels[0].mad - 2.0).abs() < f32::EPSILON,
            "F0 R MAD"
        );
        assert!(
            (stats[0].channels[1].median - 10.0).abs() < f32::EPSILON,
            "F0 G median"
        );
        assert!(
            (stats[0].channels[1].mad - 0.0).abs() < f32::EPSILON,
            "F0 G MAD"
        );
        assert!(
            (stats[0].channels[2].median - 50.0).abs() < f32::EPSILON,
            "F0 B median"
        );
        assert!(
            (stats[0].channels[2].mad - 50.0).abs() < f32::EPSILON,
            "F0 B MAD"
        );

        // Frame 1
        assert!(
            (stats[1].channels[0].median - 2.0).abs() < f32::EPSILON,
            "F1 R median"
        );
        assert!(
            (stats[1].channels[0].mad - 0.0).abs() < f32::EPSILON,
            "F1 R MAD"
        );
        assert!(
            (stats[1].channels[1].median - 2.5).abs() < f32::EPSILON,
            "F1 G median"
        );
        assert!(
            (stats[1].channels[1].mad - 1.0).abs() < f32::EPSILON,
            "F1 G MAD"
        );
        assert!(
            (stats[1].channels[2].median - 25.0).abs() < f32::EPSILON,
            "F1 B median"
        );
        assert!(
            (stats[1].channels[2].mad - 10.0).abs() < f32::EPSILON,
            "F1 B MAD"
        );
    }

    #[test]
    fn test_cache_filename_for_path() {
        // Same path should always produce same hash
        let path1 = Path::new("/some/path/image.fits");
        let filename1 = cache_filename_for_path(path1);
        let filename2 = cache_filename_for_path(path1);
        assert_eq!(filename1, filename2);

        // Different paths should produce different hashes
        let path2 = Path::new("/other/path/image.fits");
        let filename3 = cache_filename_for_path(path2);
        assert_ne!(filename1, filename3);

        // Filename should end with .bin
        assert!(filename1.ends_with(".bin"));

        // Filename should be hex (16 chars + .bin)
        assert_eq!(filename1.len(), 20); // 16 hex chars + ".bin"
    }

    #[test]
    fn test_cache_filename_deterministic_across_calls() {
        // Hash must be deterministic (FNV-1a with fixed seed). Pin a known value
        // so any accidental revert to DefaultHasher (random seed) is caught.
        let path = Path::new("/test/deterministic.fits");
        let expected = cache_filename_for_path(path);

        // Call multiple times to simulate "across invocations" within same process
        for _ in 0..10 {
            assert_eq!(
                cache_filename_for_path(path),
                expected,
                "Cache filename must be deterministic"
            );
        }

        // Pin the exact value. If someone reverts to DefaultHasher (random seed),
        // this assertion will fail because the hash changes between runs.
        assert_eq!(expected, "6f63e2eb959a4c65.bin");
        // Verify it's a valid hex filename
        let hex_part = &expected[..16];
        assert!(
            hex_part.chars().all(|c| c.is_ascii_hexdigit()),
            "Filename must be hex: {hex_part}"
        );
    }

    #[test]
    fn test_source_meta_validates_mtime() {
        let temp_dir = std::env::temp_dir().join("test_source_meta_validates");
        std::fs::create_dir_all(&temp_dir).unwrap();

        let source = temp_dir.join("source.fits");
        std::fs::write(&source, b"original data").unwrap();

        let base = "abc123.bin";

        // No meta file yet — validation should fail
        assert!(!validate_source_meta(&temp_dir, base, &source));

        // Write meta for current source
        let mtime = source_mtime(&source).unwrap();
        write_source_meta(&temp_dir, base, mtime);

        // Now validation should pass
        assert!(validate_source_meta(&temp_dir, base, &source));

        // Modify the source file (touch with new content to change mtime)
        std::thread::sleep(std::time::Duration::from_millis(1100));
        std::fs::write(&source, b"modified data").unwrap();

        // Validation should fail — source changed
        assert!(!validate_source_meta(&temp_dir, base, &source));

        // Cleanup
        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_frame_stats_sidecar_roundtrip() {
        let temp_dir = std::env::temp_dir().join("lumos_stats_roundtrip_test");
        std::fs::create_dir_all(&temp_dir).unwrap();

        let base = "test_frame.bin";

        // 1-channel stats
        let stats_1ch = FrameStats {
            channels: [ChannelStats {
                median: 42.5,
                mad: 3.25,
            }]
            .into_iter()
            .collect(),
        };
        write_frame_stats(&temp_dir, base, &stats_1ch);
        let read_1ch = read_frame_stats(&temp_dir, base).unwrap();
        assert_eq!(read_1ch.channels.len(), 1);
        assert_eq!(read_1ch.channels[0].median, 42.5);
        assert_eq!(read_1ch.channels[0].mad, 3.25);

        // 3-channel stats (overwrites the file)
        let stats_3ch = FrameStats {
            channels: [
                ChannelStats {
                    median: 100.0,
                    mad: 1.5,
                },
                ChannelStats {
                    median: 200.0,
                    mad: 2.5,
                },
                ChannelStats {
                    median: 300.0,
                    mad: 3.5,
                },
            ]
            .into_iter()
            .collect(),
        };
        write_frame_stats(&temp_dir, base, &stats_3ch);
        let read_3ch = read_frame_stats(&temp_dir, base).unwrap();
        assert_eq!(read_3ch.channels.len(), 3);
        // Verify exact f32 roundtrip for each channel
        for (i, (got, expected)) in read_3ch
            .channels
            .iter()
            .zip(stats_3ch.channels.iter())
            .enumerate()
        {
            assert_eq!(got.median, expected.median, "channel {i} median");
            assert_eq!(got.mad, expected.mad, "channel {i} mad");
        }

        // Missing file returns None
        assert!(read_frame_stats(&temp_dir, "nonexistent.bin").is_none());

        // Corrupt file returns None
        let corrupt_path = stats_path(&temp_dir, "corrupt.bin");
        std::fs::write(&corrupt_path, b"bad").unwrap();
        assert!(read_frame_stats(&temp_dir, "corrupt.bin").is_none());

        // Cleanup
        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_load_and_cache_frame_reuse_preserves_stats() {
        // Verify that stats computed on first load match stats read from sidecar on reuse.
        let temp_dir = std::env::temp_dir().join("lumos_cache_reuse_stats_test");
        std::fs::create_dir_all(&temp_dir).unwrap();

        // Non-uniform data so median and MAD are non-trivial
        let dims = ImageDimensions::new((4, 3), 1);
        // [0,1,2,3,4,5,6,7,8,9,10,11] → median=5.5, deviations=[5.5,4.5,3.5,2.5,1.5,0.5,0.5,1.5,2.5,3.5,4.5,5.5] → MAD=3.0
        let pixels: Vec<f32> = (0..12).map(|i| i as f32).collect();
        let image = AstroImage::from_pixels(dims, pixels);

        let source_path = temp_dir.join("source.tiff");
        image.save(&source_path).unwrap();

        let base_filename = "stats_test.bin";

        // First call — loads image, computes stats, writes sidecar
        let (_, first_stats) =
            load_and_cache_frame::<AstroImage>(&temp_dir, base_filename, &source_path, dims, 0)
                .unwrap();

        assert_eq!(first_stats.channels.len(), 1);
        assert!((first_stats.channels[0].median - 5.5).abs() < f32::EPSILON);
        assert!((first_stats.channels[0].mad - 3.0).abs() < f32::EPSILON);

        // Second call — reuses cache, reads stats from sidecar
        let (_, reused_stats) =
            load_and_cache_frame::<AstroImage>(&temp_dir, base_filename, &source_path, dims, 0)
                .unwrap();

        // Stats must be identical (exact f32 roundtrip via le_bytes)
        assert_eq!(reused_stats.channels.len(), first_stats.channels.len());
        assert_eq!(
            reused_stats.channels[0].median,
            first_stats.channels[0].median
        );
        assert_eq!(reused_stats.channels[0].mad, first_stats.channels[0].mad);

        // Cleanup
        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_missing_stats_sidecar_forces_reload() {
        // If the .stats file is deleted but .meta and .bin remain,
        // load_and_cache_frame should NOT reuse cache (can_reuse = false).
        let temp_dir = std::env::temp_dir().join("lumos_missing_stats_test");
        std::fs::create_dir_all(&temp_dir).unwrap();

        let dims = ImageDimensions::new((4, 3), 1);
        let pixels: Vec<f32> = (0..12).map(|i| i as f32).collect();
        let image = AstroImage::from_pixels(dims, pixels);

        let source_path = temp_dir.join("source.tiff");
        image.save(&source_path).unwrap();

        let base_filename = "missing_stats.bin";

        // First call — creates cache + sidecars
        let (_, first_stats) =
            load_and_cache_frame::<AstroImage>(&temp_dir, base_filename, &source_path, dims, 0)
                .unwrap();

        // Delete only the .stats sidecar
        let sp = stats_path(&temp_dir, base_filename);
        assert!(sp.exists());
        std::fs::remove_file(&sp).unwrap();

        // Second call — should reload (not panic) and recompute stats
        let (_, reloaded_stats) =
            load_and_cache_frame::<AstroImage>(&temp_dir, base_filename, &source_path, dims, 0)
                .unwrap();

        // Stats should match (same source image)
        assert_eq!(
            reloaded_stats.channels[0].median,
            first_stats.channels[0].median
        );
        assert_eq!(reloaded_stats.channels[0].mad, first_stats.channels[0].mad);

        // .stats file should be recreated
        assert!(sp.exists());

        // Cleanup
        let _ = std::fs::remove_dir_all(&temp_dir);
    }
}
