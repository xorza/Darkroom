//! Image cache for stacking operations.
//!
//! Supports two modes:
//! - In-memory: When all images fit in available RAM (75%), keeps images directly
//! - Disk-based: Uses memory-mapped binary files for larger datasets
//!
//! Generic over any `StackableImage` type (e.g., `AstroImage`, `CfaImage`).
//!
//! Cache format (disk mode):
//! - Each image frame has one file per channel (planar storage)
//! - File naming: `{hash}_c{channel}.bin` (e.g., `abc123_c0.bin`, `abc123_c1.bin`)
//! - Each file contains raw f32 pixels in row-major order (width * height * 4 bytes)

use std::fs::File;
use std::hash::{Hash, Hasher};
use std::io::{BufWriter, Write};
use std::mem::size_of;
use std::path::{Path, PathBuf};

use arrayvec::ArrayVec;
use common::fnv::FnvHasher;
use common::parallel::try_par_map_limited;
use memmap2::Mmap;
use rayon::prelude::*;

use crate::astro_image::{AstroImageMetadata, ImageDimensions, PixelData};
use crate::stacking::FrameType;
use crate::stacking::cache_config::{
    CacheConfig, MEMORY_PERCENT, compute_optimal_chunk_rows_with_memory,
};
use crate::stacking::error::Error;
use crate::stacking::progress::{ProgressCallback, StackingStage, report_progress};
use crate::stacking::stack::FrameNorm;

/// Per-channel robust statistics (median and MAD).
#[derive(Debug, Clone, Copy)]
pub(crate) struct ChannelStats {
    pub median: f32,
    pub mad: f32,
}

/// Per-frame statistics: one `ChannelStats` per channel.
#[derive(Debug, Clone)]
pub(crate) struct FrameStats {
    pub channels: ArrayVec<ChannelStats, 3>,
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
}

impl ScratchBuffers {
    fn new(frame_count: usize) -> Self {
        Self {
            indices: Vec::with_capacity(frame_count),
            floats_a: Vec::with_capacity(frame_count),
            floats_b: Vec::with_capacity(frame_count),
        }
    }
}

/// Trait for images that can be stacked via `ImageCache`.
///
/// Implementations must provide planar channel access as `&[f32]` slices
/// and a file-loading constructor.
pub(crate) trait StackableImage: Send + Sync + std::fmt::Debug + Sized {
    fn dimensions(&self) -> ImageDimensions;
    fn channel(&self, c: usize) -> &[f32];
    fn metadata(&self) -> &AstroImageMetadata;
    fn load(path: &Path) -> Result<Self, Error>;

    /// Construct from stacking output.
    fn from_stacked(
        pixels: PixelData,
        metadata: AstroImageMetadata,
        dimensions: ImageDimensions,
    ) -> Self;

    /// In-memory size of this image's pixel data in bytes.
    fn size_in_bytes(&self) -> usize {
        self.dimensions().sample_count() * size_of::<f32>()
    }
}

/// Generate a cache filename from the hash of the source path.
fn cache_filename_for_path(path: &Path) -> String {
    let mut hasher = FnvHasher::new();
    path.hash(&mut hasher);
    let hash = hasher.finish();
    format!("{:016x}.bin", hash)
}

/// Cached frame data for disk-backed storage.
/// Contains 1–3 memory-mapped channels (L, RGB).
#[derive(Debug)]
struct CachedFrame {
    channels: ArrayVec<Mmap, 3>,
}

/// Storage mode for image data.
#[derive(Debug)]
enum Storage<I> {
    /// All images fit in memory - stores images directly for planar channel access.
    InMemory(Vec<I>),
    /// Images stored on disk with memory-mapped access.
    /// Each frame has one file per channel for efficient planar access.
    DiskBacked {
        frames: Vec<CachedFrame>,
        cache_dir: PathBuf,
    },
}

/// Image cache that automatically chooses between in-memory and disk-based storage.
#[derive(Debug)]
pub(crate) struct ImageCache<I> {
    storage: Storage<I>,
    /// Image dimensions (same for all frames).
    dimensions: ImageDimensions,
    /// Metadata from first frame.
    metadata: AstroImageMetadata,
    /// Configuration for cache operations.
    config: CacheConfig,
    /// Progress callback.
    progress: ProgressCallback,
}

impl<I: StackableImage> ImageCache<I> {
    /// Check if images would fit in memory given available memory.
    ///
    /// Returns true if total image size fits within usable memory (75% of available).
    fn fits_in_memory(bytes_per_image: usize, frame_count: usize, available_memory: u64) -> bool {
        let Some(total_bytes) = bytes_per_image.checked_mul(frame_count) else {
            return false;
        };
        let usable_memory = available_memory * MEMORY_PERCENT / 100;
        (total_bytes as u64) <= usable_memory
    }

    /// Create cache from image paths.
    ///
    /// Automatically chooses storage mode based on available memory:
    /// - If all images fit in 75% of available RAM, keeps them in memory
    /// - Otherwise, writes to disk cache and uses memory-mapped access
    pub fn from_paths<P: AsRef<Path> + Sync>(
        paths: &[P],
        config: &CacheConfig,
        frame_type: FrameType,
        progress: ProgressCallback,
    ) -> Result<Self, Error> {
        if paths.is_empty() {
            return Err(Error::NoPaths);
        }

        // Report initial progress
        report_progress(&progress, 0, paths.len(), StackingStage::Loading);

        // Load first image to get dimensions
        let first_path = paths[0].as_ref();
        let first_image = I::load(first_path)?;
        let dimensions = first_image.dimensions();
        let metadata = first_image.metadata().clone();

        // Check available memory (from config or system)
        let available_memory = config.get_available_memory();
        let use_in_memory =
            Self::fits_in_memory(first_image.size_in_bytes(), paths.len(), available_memory);

        tracing::info!(
            frame_count = paths.len(),
            sample_count = dimensions.sample_count(),
            available_mb = available_memory / (1024 * 1024),
            use_in_memory,
            "Image cache storage decision"
        );

        let storage = if use_in_memory {
            Self::load_in_memory(paths, &progress, frame_type, dimensions, first_image)?
        } else {
            Self::load_to_disk(
                paths,
                config,
                &progress,
                frame_type,
                dimensions,
                first_image,
            )?
        };

        Ok(Self {
            storage,
            dimensions,
            metadata,
            config: config.clone(),
            progress,
        })
    }

    /// Load all images into memory.
    fn load_in_memory<P: AsRef<Path> + Sync>(
        paths: &[P],
        progress: &ProgressCallback,
        frame_type: FrameType,
        dimensions: ImageDimensions,
        first_image: I,
    ) -> Result<Storage<I>, Error> {
        report_progress(progress, 1, paths.len(), StackingStage::Loading);

        // Load remaining images in parallel, at most 3 at a time to cap memory/IO pressure
        let indexed_paths: Vec<(usize, &P)> = paths[1..]
            .iter()
            .enumerate()
            .map(|(i, p)| (i + 1, p))
            .collect();
        let remaining = try_par_map_limited(&indexed_paths, 3, |&(idx, ref path)| {
            let path_ref = path.as_ref();
            let image = I::load(path_ref)?;
            if image.dimensions() != dimensions {
                return Err(Error::DimensionMismatch {
                    frame_type,
                    index: idx,
                    expected: dimensions,
                    actual: image.dimensions(),
                });
            }
            Ok(image)
        })?;

        // Build final vector
        let mut images = Vec::with_capacity(paths.len());
        images.push(first_image);
        images.extend(remaining);

        report_progress(progress, paths.len(), paths.len(), StackingStage::Loading);

        tracing::info!("Loaded {} frames into memory", images.len());
        Ok(Storage::InMemory(images))
    }

    /// Load images to disk cache with memory-mapped access.
    /// Each channel is stored in a separate file for efficient planar access.
    /// Images are loaded and cached in parallel for better throughput.
    fn load_to_disk<P: AsRef<Path> + Sync>(
        paths: &[P],
        config: &CacheConfig,
        progress: &ProgressCallback,
        frame_type: FrameType,
        dimensions: ImageDimensions,
        first_image: I,
    ) -> Result<Storage<I>, Error> {
        let cache_dir = &config.cache_dir;
        std::fs::create_dir_all(cache_dir).map_err(|e| Error::CreateCacheDir {
            path: cache_dir.to_path_buf(),
            source: e,
        })?;

        // Cache first image
        let first_path = paths[0].as_ref();
        let base_filename = cache_filename_for_path(first_path);
        let first_cached =
            cache_image_channels(cache_dir, &base_filename, &first_image, dimensions)?;
        report_progress(progress, 1, paths.len(), StackingStage::Loading);

        // Process remaining images in parallel, at most 3 at a time to cap memory/IO pressure
        // Each frame writes to unique files (based on path hash), so no contention
        let indexed_paths: Vec<(usize, &P)> = paths[1..]
            .iter()
            .enumerate()
            .map(|(i, p)| (i + 1, p))
            .collect();
        let remaining = try_par_map_limited(&indexed_paths, 3, |&(idx, ref path)| {
            let path_ref = path.as_ref();
            let base_filename = cache_filename_for_path(path_ref);
            load_and_cache_frame::<I>(
                cache_dir,
                &base_filename,
                path_ref,
                dimensions,
                frame_type,
                idx,
            )
        })?;

        // Build final frames vector
        let mut cached_frames = Vec::with_capacity(paths.len());
        cached_frames.push(first_cached);
        cached_frames.extend(remaining);

        report_progress(progress, paths.len(), paths.len(), StackingStage::Loading);

        tracing::info!(
            "Cached {} frames ({} channels each) to disk at {:?}",
            cached_frames.len(),
            dimensions.channels,
            cache_dir
        );

        Ok(Storage::DiskBacked {
            frames: cached_frames,
            cache_dir: cache_dir.to_path_buf(),
        })
    }

    /// Get image dimensions (same for all frames).
    pub fn dimensions(&self) -> ImageDimensions {
        self.dimensions
    }

    /// Get metadata from the first frame.
    pub fn metadata(&self) -> &AstroImageMetadata {
        &self.metadata
    }

    /// Get number of cached frames.
    pub fn frame_count(&self) -> usize {
        match &self.storage {
            Storage::InMemory(images) => images.len(),
            Storage::DiskBacked { frames, .. } => frames.len(),
        }
    }

    /// Process images in horizontal chunks, applying a combine function to each pixel.
    ///
    /// Returns `PixelData` with the combined result.
    ///
    /// Optional `weights` provide per-frame weights for weighted combining.
    /// Optional `norm_params` apply per-frame affine normalization before combining.
    ///
    /// Processing is done per-channel, parallelized per-row with rayon.
    pub fn process_chunked<F>(
        &self,
        weights: Option<&[f32]>,
        norm_params: Option<&[FrameNorm]>,
        combine: F,
    ) -> PixelData
    where
        F: Fn(&mut [f32], Option<&[f32]>, &mut ScratchBuffers) -> f32 + Sync,
    {
        if let Some(w) = weights {
            assert_eq!(
                w.len(),
                self.frame_count(),
                "Weight count must match frame count"
            );
        }
        self.process_chunks_internal(|output_slice, chunks, frame_count, width, channel| {
            output_slice
                .par_chunks_mut(width)
                .enumerate()
                .for_each_init(
                    || (vec![0.0f32; frame_count], ScratchBuffers::new(frame_count)),
                    |(values, scratch), (row_in_chunk, row_output)| {
                        let row_offset = row_in_chunk * width;

                        for (pixel_in_row, out) in row_output.iter_mut().enumerate() {
                            let pixel_idx = row_offset + pixel_in_row;
                            if let Some(norms) = norm_params {
                                for (frame_idx, chunk) in chunks.iter().enumerate() {
                                    let np = norms[frame_idx].channels[channel];
                                    values[frame_idx] = chunk[pixel_idx] * np.gain + np.offset;
                                }
                            } else {
                                for (frame_idx, chunk) in chunks.iter().enumerate() {
                                    values[frame_idx] = chunk[pixel_idx];
                                }
                            }
                            *out = combine(values, weights, scratch);
                        }
                    },
                );
        })
    }

    /// Internal chunk processing - handles chunking logic, calls processor for each chunk.
    /// The closure receives `(output_slice, chunks, frame_count, width, channel)`.
    /// Returns `PixelData` with the combined result.
    fn process_chunks_internal<F>(&self, mut process_chunk: F) -> PixelData
    where
        F: FnMut(&mut [f32], &[&[f32]], usize, usize, usize),
    {
        let dims = self.dimensions;
        let frame_count = self.frame_count();
        let width = dims.width;
        let height = dims.height;
        let available_memory = self.config.get_available_memory();

        let chunk_rows = match &self.storage {
            Storage::InMemory(_) => height,
            Storage::DiskBacked { .. } => {
                compute_optimal_chunk_rows_with_memory(width, 1, frame_count, available_memory)
            }
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
                    self.read_channel_chunk(frame_idx, channel, start_row, end_row)
                }));

                let output_slice = &mut output.channel_mut(channel).pixels_mut()
                    [start_row * width..][..pixels_in_chunk];

                process_chunk(output_slice, &chunks, frame_count, width, channel);

                report_progress(
                    &self.progress,
                    channel * num_chunks + chunk_idx + 1,
                    total_work,
                    StackingStage::Processing,
                );
            }
        }

        output
    }

    /// Compute per-frame, per-channel median and MAD (robust scale).
    ///
    /// Returns one `FrameStats` per frame, each containing per-channel statistics.
    pub fn compute_channel_stats(&self) -> Vec<FrameStats> {
        let frame_count = self.frame_count();
        let channels = self.dimensions.channels;
        let height = self.dimensions.height;
        let pixel_count = self.dimensions.width * height;
        let mut stats = Vec::with_capacity(frame_count);
        let mut buf = Vec::with_capacity(pixel_count);
        let mut scratch = Vec::with_capacity(pixel_count);

        for frame_idx in 0..frame_count {
            let mut frame_channels = ArrayVec::new();
            for channel in 0..channels {
                let data = self.read_channel_chunk(frame_idx, channel, 0, height);
                buf.clear();
                buf.extend_from_slice(data);
                let median = crate::math::median_f32_mut(&mut buf);
                let mad = crate::math::mad_f32_with_scratch(data, median, &mut scratch);
                frame_channels.push(ChannelStats { median, mad });
            }
            stats.push(FrameStats {
                channels: frame_channels,
            });
        }

        stats
    }

    /// Read a horizontal chunk (rows start_row..end_row) of a single channel from a frame.
    fn read_channel_chunk(
        &self,
        frame_idx: usize,
        channel: usize,
        start_row: usize,
        end_row: usize,
    ) -> &[f32] {
        let width = self.dimensions.width;
        let start_pixel = start_row * width;
        let end_pixel = end_row * width;

        match &self.storage {
            Storage::InMemory(images) => {
                let channel_data = images[frame_idx].channel(channel);
                &channel_data[start_pixel..end_pixel]
            }
            Storage::DiskBacked { frames, .. } => {
                let mmap = &frames[frame_idx].channels[channel];
                let start_offset = start_pixel * size_of::<f32>();
                let end_offset = end_pixel * size_of::<f32>();
                let bytes = &mmap[start_offset..end_offset];
                bytemuck::cast_slice(bytes)
            }
        }
    }
}

impl<I> ImageCache<I> {
    /// Remove all cache files (only applies to disk-backed storage).
    pub fn cleanup(&self) {
        if self.config.keep_cache {
            return;
        }

        if let Storage::DiskBacked { cache_dir, .. } = &self.storage {
            let _ = std::fs::remove_dir_all(cache_dir);
        }
    }
}

impl<I> Drop for ImageCache<I> {
    fn drop(&mut self) {
        self.cleanup();
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

    let pixels_per_channel = expected_dims.width * expected_dims.height;
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
/// Returns the CachedFrame with memory-mapped channel data.
fn load_and_cache_frame<I: StackableImage>(
    cache_dir: &Path,
    base_filename: &str,
    source_path: &Path,
    dimensions: ImageDimensions,
    frame_type: FrameType,
    frame_index: usize,
) -> Result<CachedFrame, Error> {
    let channels = dimensions.channels;

    // Check if all channel files exist, have correct size, and source hasn't changed
    let meta_valid = validate_source_meta(cache_dir, base_filename, source_path);
    let can_reuse = meta_valid
        && (0..channels).all(|c| {
            let channel_path = cache_dir.join(channel_cache_filename(base_filename, c));
            try_reuse_channel_cache_file(&channel_path, dimensions)
        });

    if can_reuse {
        // Reuse existing cache files - just mmap them
        let mut cached_channels = ArrayVec::new();
        for c in 0..channels {
            let channel_path = cache_dir.join(channel_cache_filename(base_filename, c));
            cached_channels.push(mmap_channel_file(channel_path)?);
        }
        tracing::debug!(
            source = %source_path.display(),
            "Reusing existing cache files"
        );
        Ok(CachedFrame {
            channels: cached_channels,
        })
    } else {
        // Load image and write to cache
        let image = I::load(source_path)?;

        if image.dimensions() != dimensions {
            return Err(Error::DimensionMismatch {
                frame_type,
                index: frame_index,
                expected: dimensions,
                actual: image.dimensions(),
            });
        }

        let result = cache_image_channels(cache_dir, base_filename, &image, dimensions)?;

        // Record source mtime so future runs can detect stale cache
        if let Some(mtime) = source_mtime(source_path) {
            write_source_meta(cache_dir, base_filename, mtime);
        }

        Ok(result)
    }
}

/// Cache all channels of an image to separate files and return a CachedFrame.
fn cache_image_channels(
    cache_dir: &Path,
    base_filename: &str,
    image: &impl StackableImage,
    dimensions: ImageDimensions,
) -> Result<CachedFrame, Error> {
    let channels = dimensions.channels;

    let mut cached_channels = ArrayVec::new();

    for c in 0..channels {
        let channel_filename = channel_cache_filename(base_filename, c);
        let channel_path = cache_dir.join(&channel_filename);

        // Write channel if not reusable
        if !try_reuse_channel_cache_file(&channel_path, dimensions) {
            write_channel_cache_file(&channel_path, image.channel(c))?;
        }

        cached_channels.push(mmap_channel_file(channel_path)?);
    }

    Ok(CachedFrame {
        channels: cached_channels,
    })
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::astro_image::AstroImage;

    /// Create an in-memory ImageCache from loaded images (test/internal helper).
    #[cfg(test)]
    pub(crate) fn make_test_cache<I: StackableImage>(images: Vec<I>) -> ImageCache<I> {
        let dimensions = images[0].dimensions();
        let metadata = images[0].metadata().clone();
        ImageCache {
            storage: Storage::InMemory(images),
            dimensions,
            metadata,
            config: CacheConfig::default(),
            progress: ProgressCallback::default(),
        }
    }

    // ========== Storage Type Selection Tests ==========

    #[test]
    fn test_fits_in_memory() {
        // Test basic fit: 10 images of 1000x1000x3 = 120MB, 1GB available (750MB usable)
        assert!(ImageCache::<AstroImage>::fits_in_memory(
            1000 * 1000 * 3 * 4,
            10,
            1024 * 1024 * 1024
        ));

        // Test doesn't fit: 100 images of 6000x4000x3 = 28.8GB, 16GB available (12GB usable)
        assert!(!ImageCache::<AstroImage>::fits_in_memory(
            6000 * 4000 * 3 * 4,
            100,
            16 * 1024 * 1024 * 1024
        ));

        // Test boundary: exactly at 75% threshold
        let bytes_per_image = 1000 * 1000 * 4;
        let frame_count = 10;
        let bytes_needed = (bytes_per_image * frame_count) as u64;
        let available_at_boundary = (bytes_needed * 100).div_ceil(75);
        assert!(ImageCache::<AstroImage>::fits_in_memory(
            bytes_per_image,
            frame_count,
            available_at_boundary
        ));
        assert!(!ImageCache::<AstroImage>::fits_in_memory(
            bytes_per_image,
            frame_count,
            available_at_boundary - 2
        ));

        // Test grayscale vs RGB with same memory
        let available = 4 * 1024 * 1024 * 1024u64; // 4GB (3GB usable)
        assert!(ImageCache::<AstroImage>::fits_in_memory(
            6000 * 4000 * 4,
            20,
            available
        )); // Grayscale: 1.92GB
        assert!(!ImageCache::<AstroImage>::fits_in_memory(
            6000 * 4000 * 3 * 4,
            20,
            available
        )); // RGB: 5.76GB
    }

    // ========== Cache File Tests ==========

    #[test]
    fn test_channel_cache_roundtrip() {
        let temp_dir = std::env::temp_dir().join("lumos_channel_cache_test");
        std::fs::create_dir_all(&temp_dir).unwrap();

        let dims = ImageDimensions::new(4, 3, 3);
        let pixels: Vec<f32> = (0..36).map(|i| i as f32).collect();
        let image = AstroImage::from_pixels(dims, pixels);

        let base_filename = "test_frame.bin";
        let cached_frame = cache_image_channels(&temp_dir, base_filename, &image, dims).unwrap();

        // Should create 3 channel files
        assert_eq!(cached_frame.channels.len(), 3);

        // Verify each channel file contains the correct data
        for (c, cached_channel) in cached_frame.channels.iter().enumerate() {
            let read_channel: &[f32] = bytemuck::cast_slice(&cached_channel[..]);
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

        let dims = ImageDimensions::new(4, 3, 1);
        let pixels: Vec<f32> = (0..12).map(|i| i as f32).collect();
        let image = AstroImage::from_pixels(dims, pixels);

        let cache_path = temp_dir.join("reuse_c0.bin");
        write_channel_cache_file(&cache_path, image.channel(0)).unwrap();

        // Matching dimensions: reusable
        assert!(try_reuse_channel_cache_file(&cache_path, dims));

        // Different dimensions: not reusable
        assert!(!try_reuse_channel_cache_file(
            &cache_path,
            ImageDimensions::new(8, 3, 1)
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
        let result = ImageCache::<AstroImage>::from_paths(
            &Vec::<PathBuf>::new(),
            &config,
            FrameType::Dark,
            ProgressCallback::default(),
        );
        assert!(matches!(result.unwrap_err(), Error::NoPaths));

        // Nonexistent file
        let result = ImageCache::<AstroImage>::from_paths(
            &[PathBuf::from("/nonexistent/path/image.fits")],
            &config,
            FrameType::Dark,
            ProgressCallback::default(),
        );
        assert!(matches!(result.unwrap_err(), Error::ImageLoad { .. }));
    }

    // ========== Processing Tests ==========

    #[test]
    fn test_process_chunked_median() {
        // Create in-memory cache with 3 grayscale frames
        let dims = ImageDimensions::new(4, 4, 1);
        let images = vec![
            AstroImage::from_pixels(dims, vec![1.0; 16]),
            AstroImage::from_pixels(dims, vec![3.0; 16]),
            AstroImage::from_pixels(dims, vec![2.0; 16]),
        ];

        let cache = make_test_cache(images);

        // Median of [1, 3, 2] = 2
        let result = cache.process_chunked(None, None, |values, _, _| {
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
        let dims = ImageDimensions::new(2, 2, 3);
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
        let result = cache.process_chunked(None, None, |values, _, _| {
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
        let dims = ImageDimensions::new(2, 2, 1);
        let images = vec![
            AstroImage::from_pixels(dims, vec![10.0; 4]),
            AstroImage::from_pixels(dims, vec![20.0; 4]),
        ];

        let cache = make_test_cache(images);

        // Weighted mean with weights [1, 3]: (10*1 + 20*3) / (1+3) = 70/4 = 17.5
        let weights = vec![1.0, 3.0];
        let result = cache.process_chunked(Some(&weights), None, |values, w, _| {
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
    fn test_frame_count() {
        let dims = ImageDimensions::new(2, 2, 1);
        let images = vec![
            AstroImage::from_pixels(dims, vec![1.0; 4]),
            AstroImage::from_pixels(dims, vec![2.0; 4]),
            AstroImage::from_pixels(dims, vec![3.0; 4]),
        ];

        let cache = make_test_cache(images);

        assert_eq!(cache.frame_count(), 3);
    }

    #[test]
    fn test_cleanup_removes_files() {
        let temp_dir = std::env::temp_dir().join("lumos_cleanup_test");
        std::fs::create_dir_all(&temp_dir).unwrap();

        // Create a real cached frame using cache_image_channels
        let dims = ImageDimensions::new(2, 2, 3);
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

        let cache: ImageCache<AstroImage> = ImageCache {
            storage: Storage::DiskBacked {
                frames: vec![cached_frame],
                cache_dir: temp_dir.clone(),
            },
            dimensions: dims,
            metadata: AstroImageMetadata::default(),
            config,
            progress: ProgressCallback::default(),
        };

        // Drop the cache - should trigger cleanup via Drop
        drop(cache);

        // Entire cache directory should be removed
        assert!(
            !temp_dir.exists(),
            "Cache directory should be deleted on cleanup"
        );
    }

    #[test]
    fn test_read_channel_chunk_in_memory() {
        let dims = ImageDimensions::new(4, 3, 1);
        // Pixels 0-11 in row-major order
        let pixels: Vec<f32> = (0..12).map(|i| i as f32).collect();
        let images = vec![AstroImage::from_pixels(dims, pixels)];

        let cache = make_test_cache(images);

        // Read row 1 (pixels 4-7)
        let chunk = cache.read_channel_chunk(0, 0, 1, 2);
        let expected: Vec<f32> = (4..8).map(|i| i as f32).collect();
        assert_eq!(chunk, &expected[..]);

        // Read all rows
        let all = cache.read_channel_chunk(0, 0, 0, 3);
        assert_eq!(all.len(), 12);
    }

    #[test]
    fn test_read_channel_chunk_disk_backed() {
        let temp_dir = std::env::temp_dir().join("lumos_read_chunk_disk_test");
        std::fs::create_dir_all(&temp_dir).unwrap();

        let dims = ImageDimensions::new(4, 3, 1);
        let pixels: Vec<f32> = (0..12).map(|i| i as f32).collect();
        let image = AstroImage::from_pixels(dims, pixels);

        // Cache the image to disk
        let base_filename = "test_chunk.bin";
        let cached_frame = cache_image_channels(&temp_dir, base_filename, &image, dims).unwrap();

        let cache: ImageCache<AstroImage> = ImageCache {
            storage: Storage::DiskBacked {
                frames: vec![cached_frame],
                cache_dir: temp_dir.clone(),
            },
            dimensions: dims,
            metadata: AstroImageMetadata::default(),
            config: CacheConfig::default(),
            progress: ProgressCallback::default(),
        };

        // Read row 1 (pixels 4-7)
        let chunk = cache.read_channel_chunk(0, 0, 1, 2);
        let expected: Vec<f32> = (4..8).map(|i| i as f32).collect();
        assert_eq!(chunk, &expected[..]);

        // Read all rows
        let all = cache.read_channel_chunk(0, 0, 0, 3);
        assert_eq!(all.len(), 12);
        for (i, &val) in all.iter().enumerate() {
            assert!((val - i as f32).abs() < f32::EPSILON);
        }

        // Cleanup
        cache.cleanup();
    }

    #[test]
    fn test_frame_count_disk_backed() {
        let temp_dir = std::env::temp_dir().join("lumos_frame_count_disk_test");
        std::fs::create_dir_all(&temp_dir).unwrap();

        let dims = ImageDimensions::new(2, 2, 1);

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

        let cache: ImageCache<AstroImage> = ImageCache {
            storage: Storage::DiskBacked {
                frames,
                cache_dir: temp_dir.clone(),
            },
            dimensions: dims,
            metadata: AstroImageMetadata::default(),
            config: CacheConfig::default(),
            progress: ProgressCallback::default(),
        };

        assert_eq!(cache.frame_count(), 3);

        // Cleanup
        cache.cleanup();
    }

    #[test]
    fn test_load_and_cache_frame_fresh() {
        let temp_dir = std::env::temp_dir().join("lumos_load_cache_fresh_test");
        std::fs::create_dir_all(&temp_dir).unwrap();

        let dims = ImageDimensions::new(4, 3, 1);
        let pixels: Vec<f32> = (0..12).map(|i| i as f32).collect();
        let image = AstroImage::from_pixels(dims, pixels.clone());

        // Write a temp TIFF file to load from
        let source_path = temp_dir.join("source.tiff");
        image.save(&source_path).unwrap();

        let base_filename = "cached_frame.bin";

        // First call should load and cache
        let cached_frame = load_and_cache_frame::<AstroImage>(
            &temp_dir,
            base_filename,
            &source_path,
            dims,
            FrameType::Light,
            0,
        )
        .unwrap();

        assert_eq!(cached_frame.channels.len(), 1);

        // Verify cached data matches original
        let cached_data: &[f32] = bytemuck::cast_slice(&cached_frame.channels[0][..]);
        assert_eq!(cached_data, &pixels[..]);

        // Cleanup
        drop(cached_frame);
        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_load_and_cache_frame_reuse() {
        let temp_dir = std::env::temp_dir().join("lumos_load_cache_reuse_test");
        std::fs::create_dir_all(&temp_dir).unwrap();

        let dims = ImageDimensions::new(4, 3, 1);
        let pixels: Vec<f32> = (0..12).map(|i| i as f32).collect();
        let image = AstroImage::from_pixels(dims, pixels.clone());

        // Write a temp TIFF file
        let source_path = temp_dir.join("source.tiff");
        image.save(&source_path).unwrap();

        let base_filename = "cached_frame.bin";

        // First call - creates cache
        let first_frame = load_and_cache_frame::<AstroImage>(
            &temp_dir,
            base_filename,
            &source_path,
            dims,
            FrameType::Light,
            0,
        )
        .unwrap();

        // Second call - should reuse cache
        let second_frame = load_and_cache_frame::<AstroImage>(
            &temp_dir,
            base_filename,
            &source_path,
            dims,
            FrameType::Light,
            0,
        )
        .unwrap();

        // Both should have same data
        let first_data: &[f32] = bytemuck::cast_slice(&first_frame.channels[0][..]);
        let second_data: &[f32] = bytemuck::cast_slice(&second_frame.channels[0][..]);
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
        let actual_dims = ImageDimensions::new(4, 3, 1);
        let pixels: Vec<f32> = (0..12).map(|i| i as f32).collect();
        let image = AstroImage::from_pixels(actual_dims, pixels);

        let source_path = temp_dir.join("source.tiff");
        image.save(&source_path).unwrap();

        // Try to load with wrong expected dimensions
        let expected_dims = ImageDimensions::new(8, 6, 1);
        let result = load_and_cache_frame::<AstroImage>(
            &temp_dir,
            "cached.bin",
            &source_path,
            expected_dims,
            FrameType::Light,
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
        let dims = ImageDimensions::new(3, 3, 1);

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
        let stats = cache.compute_channel_stats();

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
        let dims = ImageDimensions::new(2, 2, 3);

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
        let stats = cache.compute_channel_stats();

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
}
