//! Image cache for stacking operations.
//!
//! Supports two modes:
//! - In-memory: When all images fit in available RAM (75%), keeps AstroImages directly
//! - Disk-based: Uses memory-mapped binary files for larger datasets
//!
//! Cache format (disk mode):
//! - Each image frame has one file per channel (planar storage)
//! - File naming: `{hash}_c{channel}.bin` (e.g., `abc123_c0.bin`, `abc123_c1.bin`)
//! - Each file contains raw f32 pixels in row-major order (width * height * 4 bytes)

use std::collections::hash_map::DefaultHasher;
use std::fs::File;
use std::hash::{Hash, Hasher};
use std::io::{self, BufWriter, Write};
use std::mem::size_of;
use std::path::{Path, PathBuf};

use memmap2::Mmap;
use rayon::prelude::*;

use crate::astro_image::{AstroImage, AstroImageMetadata, ImageDimensions};
use crate::stacking::FrameType;
use crate::stacking::cache_config::{
    CacheConfig, MEMORY_PERCENT, compute_optimal_chunk_rows_with_memory,
};
use crate::stacking::error::Error;
use crate::stacking::progress::{ProgressCallback, StackingStage, report_progress};

/// Generate a cache filename from the hash of the source path.
fn cache_filename_for_path(path: &Path) -> String {
    let mut hasher = DefaultHasher::new();
    path.hash(&mut hasher);
    let hash = hasher.finish();
    format!("{:016x}.bin", hash)
}

/// Cached frame data for disk-backed storage.
#[derive(Debug)]
struct CachedFrame {
    /// Memory-mapped channel data, one per channel.
    mmaps: Vec<Mmap>,
    /// Paths to channel cache files, one per channel.
    paths: Vec<PathBuf>,
}

/// Storage mode for image data.
#[derive(Debug)]
enum Storage {
    /// All images fit in memory - stores AstroImage directly for planar channel access.
    InMemory(Vec<AstroImage>),
    /// Images stored on disk with memory-mapped access.
    /// Each frame has one file per channel for efficient planar access.
    DiskBacked {
        frames: Vec<CachedFrame>,
        cache_dir: PathBuf,
    },
}

/// Image cache that automatically chooses between in-memory and disk-based storage.
#[derive(Debug)]
pub struct ImageCache {
    storage: Storage,
    /// Image dimensions (same for all frames).
    dimensions: ImageDimensions,
    /// Metadata from first frame.
    metadata: AstroImageMetadata,
    /// Configuration for cache operations.
    config: CacheConfig,
    /// Progress callback.
    progress: ProgressCallback,
}

impl ImageCache {
    /// Check if images would fit in memory given available memory.
    ///
    /// Returns true if total image size fits within usable memory (75% of available).
    ///
    /// Uses checked arithmetic to handle pathologically large datasets gracefully.
    fn fits_in_memory(pixel_count: usize, frame_count: usize, available_memory: u64) -> bool {
        let Some(bytes_per_image) = pixel_count.checked_mul(size_of::<f32>()) else {
            return false; // Overflow means definitely doesn't fit
        };
        let Some(total_bytes_needed) = bytes_per_image.checked_mul(frame_count) else {
            return false; // Overflow means definitely doesn't fit
        };
        let usable_memory = available_memory * MEMORY_PERCENT / 100;
        (total_bytes_needed as u64) <= usable_memory
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
        let first_image = AstroImage::from_file(first_path).map_err(|e| Error::ImageLoad {
            path: first_path.to_path_buf(),
            source: io::Error::other(e.to_string()),
        })?;
        let dimensions = first_image.dimensions();
        let metadata = first_image.metadata.clone();

        // Check available memory (from config or system)
        let available_memory = config.get_available_memory();
        let use_in_memory =
            Self::fits_in_memory(dimensions.pixel_count(), paths.len(), available_memory);

        tracing::info!(
            frame_count = paths.len(),
            pixel_count = dimensions.pixel_count(),
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
        first_image: AstroImage,
    ) -> Result<Storage, Error> {
        report_progress(progress, 1, paths.len(), StackingStage::Loading);

        // Load remaining images in parallel
        let remaining_results: Result<Vec<(usize, AstroImage)>, Error> = paths[1..]
            .par_iter()
            .enumerate()
            .map(|(i, path)| {
                let actual_index = i + 1;
                let path_ref = path.as_ref();
                let image = AstroImage::from_file(path_ref).map_err(|e| Error::ImageLoad {
                    path: path_ref.to_path_buf(),
                    source: io::Error::other(e.to_string()),
                })?;

                if image.dimensions() != dimensions {
                    return Err(Error::DimensionMismatch {
                        frame_type,
                        index: actual_index,
                        expected: dimensions,
                        actual: image.dimensions(),
                    });
                }

                Ok((actual_index, image))
            })
            .collect();

        let mut remaining = remaining_results?;

        // Sort by index to maintain order
        remaining.sort_by_key(|(i, _)| *i);

        // Build final vector of AstroImages
        let mut images = Vec::with_capacity(paths.len());
        images.push(first_image);
        images.extend(remaining.into_iter().map(|(_, img)| img));

        report_progress(progress, paths.len(), paths.len(), StackingStage::Loading);

        tracing::info!("Loaded {} frames into memory", images.len());
        Ok(Storage::InMemory(images))
    }

    /// Load images to disk cache with memory-mapped access.
    /// Each channel is stored in a separate file for efficient planar access.
    fn load_to_disk<P: AsRef<Path>>(
        paths: &[P],
        config: &CacheConfig,
        progress: &ProgressCallback,
        frame_type: FrameType,
        dimensions: ImageDimensions,
        first_image: AstroImage,
    ) -> Result<Storage, Error> {
        let cache_dir = &config.cache_dir;
        std::fs::create_dir_all(cache_dir).map_err(|e| Error::CreateCacheDir {
            path: cache_dir.to_path_buf(),
            source: e,
        })?;

        let channels = dimensions.channels;
        let mut cached_frames: Vec<CachedFrame> = Vec::with_capacity(paths.len());

        // Write first image (or reuse existing cache)
        let first_path = paths[0].as_ref();
        let base_filename = cache_filename_for_path(first_path);
        tracing::info!(
            source = %first_path.display(),
            cache_base = %base_filename,
            "Mapping source to cache files"
        );

        let cached_frame =
            cache_image_channels(cache_dir, &base_filename, &first_image, dimensions)?;
        cached_frames.push(cached_frame);
        report_progress(progress, 1, paths.len(), StackingStage::Loading);

        // Process remaining images
        for (i, path) in paths.iter().enumerate().skip(1) {
            let path_ref = path.as_ref();
            let base_filename = cache_filename_for_path(path_ref);
            tracing::info!(
                source = %path_ref.display(),
                cache_base = %base_filename,
                "Mapping source to cache files"
            );

            // Check if all channel files exist and can be reused
            let can_reuse = (0..channels).all(|c| {
                let channel_path = cache_dir.join(channel_cache_filename(&base_filename, c));
                try_reuse_channel_cache_file(&channel_path, dimensions)
            });

            if can_reuse {
                // Reuse existing cache files
                let mut frame_mmaps = Vec::with_capacity(channels);
                let mut frame_paths = Vec::with_capacity(channels);
                for c in 0..channels {
                    let channel_path = cache_dir.join(channel_cache_filename(&base_filename, c));
                    let file = File::open(&channel_path).map_err(|e| Error::OpenCacheFile {
                        path: channel_path.clone(),
                        source: e,
                    })?;
                    let mmap = unsafe {
                        Mmap::map(&file).map_err(|e| Error::MmapCacheFile {
                            path: channel_path.clone(),
                            source: e,
                        })?
                    };
                    frame_mmaps.push(mmap);
                    frame_paths.push(channel_path);
                }
                cached_frames.push(CachedFrame {
                    mmaps: frame_mmaps,
                    paths: frame_paths,
                });
            } else {
                // Load and cache the image
                let image = AstroImage::from_file(path_ref).map_err(|e| Error::ImageLoad {
                    path: path_ref.to_path_buf(),
                    source: io::Error::other(e.to_string()),
                })?;

                if image.dimensions() != dimensions {
                    return Err(Error::DimensionMismatch {
                        frame_type,
                        index: i,
                        expected: dimensions,
                        actual: image.dimensions(),
                    });
                }

                let cached_frame =
                    cache_image_channels(cache_dir, &base_filename, &image, dimensions)?;
                cached_frames.push(cached_frame);
            }

            report_progress(progress, i + 1, paths.len(), StackingStage::Loading);
        }

        tracing::info!(
            "Cached {} frames ({} channels each) to disk at {:?}",
            cached_frames.len(),
            channels,
            cache_dir
        );

        Ok(Storage::DiskBacked {
            frames: cached_frames,
            cache_dir: cache_dir.to_path_buf(),
        })
    }

    /// Get number of cached frames.
    pub fn frame_count(&self) -> usize {
        match &self.storage {
            Storage::InMemory(images) => images.len(),
            Storage::DiskBacked { frames, .. } => frames.len(),
        }
    }

    /// Remove all cache files (only applies to disk-backed storage).
    pub fn cleanup(&self) {
        if let Storage::DiskBacked { frames, cache_dir } = &self.storage {
            for frame in frames {
                for path in &frame.paths {
                    let _ = std::fs::remove_file(path);
                }
            }
            let _ = std::fs::remove_dir(cache_dir);
        }
    }

    /// Process images in horizontal chunks, applying a combine function to each pixel.
    ///
    /// This is the core processing loop shared by median and sigma-clipped stacking.
    /// Each thread gets its own buffer to avoid per-pixel allocation.
    /// The combine function receives a mutable slice to allow in-place operations.
    ///
    /// For in-memory storage, processes the entire image in one pass.
    /// For disk-backed storage, chunk size is computed adaptively based on available memory.
    /// Processing is done per-channel for efficient planar storage access.
    pub fn process_chunked<F>(&self, combine: F) -> AstroImage
    where
        F: Fn(&mut [f32]) -> f32 + Sync,
    {
        let dims = self.dimensions;
        let frame_count = self.frame_count();
        let width = dims.width;
        let height = dims.height;
        let channels = dims.channels;
        let pixels_per_channel = width * height;
        let available_memory = self.config.get_available_memory();

        // For in-memory, process entire channel; for disk, use adaptive chunking
        let chunk_rows = match &self.storage {
            Storage::InMemory(_) => height, // Process all at once
            Storage::DiskBacked { .. } => compute_optimal_chunk_rows_with_memory(
                width,
                1, // Processing one channel at a time
                frame_count,
                available_memory,
            ),
        };

        // Allocate output channels
        let mut output_channels: Vec<Vec<f32>> = (0..channels)
            .map(|_| vec![0.0f32; pixels_per_channel])
            .collect();

        let num_chunks = height.div_ceil(chunk_rows);
        let total_work = num_chunks * channels;

        report_progress(&self.progress, 0, total_work, StackingStage::Processing);

        // Process each channel independently
        for (channel, output_channel) in output_channels.iter_mut().enumerate() {
            for chunk_idx in 0..num_chunks {
                let start_row = chunk_idx * chunk_rows;
                let end_row = (start_row + chunk_rows).min(height);
                let rows_in_chunk = end_row - start_row;
                let pixels_in_chunk = rows_in_chunk * width;

                // Get channel slices from all frames
                let chunks: Vec<&[f32]> = (0..frame_count)
                    .map(|frame_idx| {
                        self.read_channel_chunk(frame_idx, channel, start_row, end_row)
                    })
                    .collect();

                let output_slice = &mut output_channel[start_row * width..][..pixels_in_chunk];

                output_slice.par_chunks_mut(width).enumerate().for_each(
                    |(row_in_chunk, row_output)| {
                        // One buffer per thread, reused for all pixels in row
                        let mut values = vec![0.0f32; frame_count];
                        let row_offset = row_in_chunk * width;

                        for (pixel_in_row, out) in row_output.iter_mut().enumerate() {
                            let pixel_idx = row_offset + pixel_in_row;
                            // Gather values from all frames
                            for (frame_idx, chunk) in chunks.iter().enumerate() {
                                values[frame_idx] = chunk[pixel_idx];
                            }
                            *out = combine(&mut values);
                        }
                    },
                );

                report_progress(
                    &self.progress,
                    channel * num_chunks + chunk_idx + 1,
                    total_work,
                    StackingStage::Processing,
                );
            }
        }

        // Build result from planar channels
        let mut result = AstroImage::from_planar_channels(dims, output_channels);
        result.metadata = self.metadata.clone();
        result
    }

    /// Process images in horizontal chunks with per-frame weights.
    ///
    /// Similar to `process_chunked`, but the combine function also receives weights.
    /// Used for weighted mean stacking with optional rejection.
    /// Processing is done per-channel for efficient planar storage access.
    pub fn process_chunked_weighted<F>(&self, weights: &[f32], combine: F) -> AstroImage
    where
        F: Fn(&mut [f32], &[f32]) -> f32 + Sync,
    {
        let dims = self.dimensions;
        let frame_count = self.frame_count();
        let width = dims.width;
        let height = dims.height;
        let channels = dims.channels;
        let pixels_per_channel = width * height;
        let available_memory = self.config.get_available_memory();

        assert_eq!(
            weights.len(),
            frame_count,
            "Weight count must match frame count"
        );

        let chunk_rows = match &self.storage {
            Storage::InMemory(_) => height,
            Storage::DiskBacked { .. } => compute_optimal_chunk_rows_with_memory(
                width,
                1, // Processing one channel at a time
                frame_count,
                available_memory,
            ),
        };

        // Allocate output channels
        let mut output_channels: Vec<Vec<f32>> = (0..channels)
            .map(|_| vec![0.0f32; pixels_per_channel])
            .collect();

        let num_chunks = height.div_ceil(chunk_rows);
        let total_work = num_chunks * channels;

        report_progress(&self.progress, 0, total_work, StackingStage::Processing);

        // Process each channel independently
        for (channel, output_channel) in output_channels.iter_mut().enumerate() {
            for chunk_idx in 0..num_chunks {
                let start_row = chunk_idx * chunk_rows;
                let end_row = (start_row + chunk_rows).min(height);
                let rows_in_chunk = end_row - start_row;
                let pixels_in_chunk = rows_in_chunk * width;

                let chunks: Vec<&[f32]> = (0..frame_count)
                    .map(|frame_idx| {
                        self.read_channel_chunk(frame_idx, channel, start_row, end_row)
                    })
                    .collect();

                let output_slice = &mut output_channel[start_row * width..][..pixels_in_chunk];

                output_slice.par_chunks_mut(width).enumerate().for_each(
                    |(row_in_chunk, row_output)| {
                        let mut values = vec![0.0f32; frame_count];
                        let mut local_weights = weights.to_vec();
                        let row_offset = row_in_chunk * width;

                        for (pixel_in_row, out) in row_output.iter_mut().enumerate() {
                            let pixel_idx = row_offset + pixel_in_row;
                            for (frame_idx, chunk) in chunks.iter().enumerate() {
                                values[frame_idx] = chunk[pixel_idx];
                            }
                            // Reset weights for each pixel (rejection may modify values array)
                            local_weights.copy_from_slice(weights);
                            *out = combine(&mut values, &local_weights);
                        }
                    },
                );

                report_progress(
                    &self.progress,
                    channel * num_chunks + chunk_idx + 1,
                    total_work,
                    StackingStage::Processing,
                );
            }
        }

        // Build result from planar channels
        let mut result = AstroImage::from_planar_channels(dims, output_channels);
        result.metadata = self.metadata.clone();
        result
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
                let mmap = &frames[frame_idx].mmaps[channel];
                let start_offset = start_pixel * size_of::<f32>();
                let end_offset = end_pixel * size_of::<f32>();
                let bytes = &mmap[start_offset..end_offset];
                bytemuck::cast_slice(bytes)
            }
        }
    }
}

/// Generate cache filename for a specific channel.
fn channel_cache_filename(base_filename: &str, channel: usize) -> String {
    // Replace .bin with _c{channel}.bin
    let stem = base_filename.trim_end_matches(".bin");
    format!("{}_c{}.bin", stem, channel)
}

/// Try to reuse an existing channel cache file if it exists and has matching size.
///
/// Returns true if the file can be reused, false if it needs to be rewritten.
fn try_reuse_channel_cache_file(path: &Path, expected_dims: ImageDimensions) -> bool {
    let Ok(file) = File::open(path) else {
        return false;
    };

    // Check file size matches expected (raw f32 data, no header)
    let pixels_per_channel = expected_dims.width * expected_dims.height;
    let expected_size = pixels_per_channel * size_of::<f32>();
    let Ok(metadata) = file.metadata() else {
        return false;
    };
    if metadata.len() != expected_size as u64 {
        return false;
    }

    tracing::debug!("Reusing existing channel cache file: {:?}", path);
    true
}

/// Write a single channel to a binary cache file.
fn write_channel_cache_file(path: &Path, channel_data: &[f32]) -> Result<(), Error> {
    let file = File::create(path).map_err(|e| Error::CreateCacheFile {
        path: path.to_path_buf(),
        source: e,
    })?;
    let mut writer = BufWriter::new(file);

    // Write raw f32 data (no header needed - dimensions tracked at ImageCache level)
    let bytes: &[u8] = bytemuck::cast_slice(channel_data);
    writer.write_all(bytes).map_err(|e| Error::WriteCacheFile {
        path: path.to_path_buf(),
        source: e,
    })?;

    writer.flush().map_err(|e| Error::WriteCacheFile {
        path: path.to_path_buf(),
        source: e,
    })?;

    Ok(())
}

/// Cache all channels of an image to separate files and return a CachedFrame.
fn cache_image_channels(
    cache_dir: &Path,
    base_filename: &str,
    image: &AstroImage,
    dimensions: ImageDimensions,
) -> Result<CachedFrame, Error> {
    let channels = dimensions.channels;
    let mut mmaps = Vec::with_capacity(channels);
    let mut paths = Vec::with_capacity(channels);

    for c in 0..channels {
        let channel_filename = channel_cache_filename(base_filename, c);
        let channel_path = cache_dir.join(&channel_filename);

        // Write channel if not reusable
        if !try_reuse_channel_cache_file(&channel_path, dimensions) {
            write_channel_cache_file(&channel_path, image.channel(c))?;
        }

        // Memory-map the channel file
        let file = File::open(&channel_path).map_err(|e| Error::OpenCacheFile {
            path: channel_path.clone(),
            source: e,
        })?;
        let mmap = unsafe {
            Mmap::map(&file).map_err(|e| Error::MmapCacheFile {
                path: channel_path.clone(),
                source: e,
            })?
        };

        mmaps.push(mmap);
        paths.push(channel_path);
    }

    Ok(CachedFrame { mmaps, paths })
}

#[cfg(test)]
mod tests {
    use super::*;

    // ========== Storage Type Selection Tests ==========

    #[test]
    fn test_fits_in_memory() {
        // Test basic fit: 10 images of 1000x1000x3 = 120MB, 1GB available (750MB usable)
        assert!(ImageCache::fits_in_memory(
            1000 * 1000 * 3,
            10,
            1024 * 1024 * 1024
        ));

        // Test doesn't fit: 100 images of 6000x4000x3 = 28.8GB, 16GB available (12GB usable)
        assert!(!ImageCache::fits_in_memory(
            6000 * 4000 * 3,
            100,
            16 * 1024 * 1024 * 1024
        ));

        // Test boundary: exactly at 75% threshold
        let pixel_count = 1000 * 1000;
        let frame_count = 10;
        let bytes_needed = (pixel_count * frame_count * 4) as u64;
        let available_at_boundary = (bytes_needed * 100).div_ceil(75);
        assert!(ImageCache::fits_in_memory(
            pixel_count,
            frame_count,
            available_at_boundary
        ));
        assert!(!ImageCache::fits_in_memory(
            pixel_count,
            frame_count,
            available_at_boundary - 2
        ));

        // Test grayscale vs RGB with same memory
        let available = 4 * 1024 * 1024 * 1024u64; // 4GB (3GB usable)
        assert!(ImageCache::fits_in_memory(6000 * 4000, 20, available)); // Grayscale: 1.92GB
        assert!(!ImageCache::fits_in_memory(6000 * 4000 * 3, 20, available)); // RGB: 5.76GB
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
        assert_eq!(cached_frame.mmaps.len(), 3);
        assert_eq!(cached_frame.paths.len(), 3);

        // Verify each channel file contains the correct data
        for (c, mmap) in cached_frame.mmaps.iter().enumerate() {
            let read_channel: &[f32] = bytemuck::cast_slice(&mmap[..]);
            let expected_channel = image.channel(c);
            assert_eq!(read_channel, expected_channel);
        }

        // Cleanup
        for path in cached_frame.paths {
            let _ = std::fs::remove_file(path);
        }
        let _ = std::fs::remove_dir(&temp_dir);
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
        let result = ImageCache::from_paths(
            &Vec::<PathBuf>::new(),
            &config,
            FrameType::Dark,
            ProgressCallback::default(),
        );
        assert!(matches!(result.unwrap_err(), Error::NoPaths));

        // Nonexistent file
        let result = ImageCache::from_paths(
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

        let cache = ImageCache {
            storage: Storage::InMemory(images),
            dimensions: dims,
            metadata: AstroImageMetadata::default(),
            config: CacheConfig::default(),
            progress: ProgressCallback::default(),
        };

        // Median of [1, 3, 2] = 2
        let result = cache.process_chunked(|values| {
            values.sort_by(|a, b| a.partial_cmp(b).unwrap());
            values[values.len() / 2]
        });

        assert_eq!(result.width(), 4);
        assert_eq!(result.height(), 4);
        assert_eq!(result.channels(), 1);
        for &pixel in result.channel(0) {
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

        let cache = ImageCache {
            storage: Storage::InMemory(images),
            dimensions: dims,
            metadata: AstroImageMetadata::default(),
            config: CacheConfig::default(),
            progress: ProgressCallback::default(),
        };

        // Mean: R=(1+5)/2=3, G=(2+6)/2=4, B=(3+7)/2=5
        let result =
            cache.process_chunked(|values| values.iter().sum::<f32>() / values.len() as f32);

        assert_eq!(result.channels(), 3);
        for &pixel in result.channel(0) {
            assert!((pixel - 3.0).abs() < f32::EPSILON, "R channel");
        }
        for &pixel in result.channel(1) {
            assert!((pixel - 4.0).abs() < f32::EPSILON, "G channel");
        }
        for &pixel in result.channel(2) {
            assert!((pixel - 5.0).abs() < f32::EPSILON, "B channel");
        }
    }

    #[test]
    fn test_process_chunked_weighted() {
        let dims = ImageDimensions::new(2, 2, 1);
        let images = vec![
            AstroImage::from_pixels(dims, vec![10.0; 4]),
            AstroImage::from_pixels(dims, vec![20.0; 4]),
        ];

        let cache = ImageCache {
            storage: Storage::InMemory(images),
            dimensions: dims,
            metadata: AstroImageMetadata::default(),
            config: CacheConfig::default(),
            progress: ProgressCallback::default(),
        };

        // Weighted mean with weights [1, 3]: (10*1 + 20*3) / (1+3) = 70/4 = 17.5
        let weights = vec![1.0, 3.0];
        let result = cache.process_chunked_weighted(&weights, |values, w| {
            let sum: f32 = values.iter().zip(w.iter()).map(|(v, wt)| v * wt).sum();
            let weight_sum: f32 = w.iter().sum();
            sum / weight_sum
        });

        for &pixel in result.channel(0) {
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

        let cache = ImageCache {
            storage: Storage::InMemory(images),
            dimensions: dims,
            metadata: AstroImageMetadata::default(),
            config: CacheConfig::default(),
            progress: ProgressCallback::default(),
        };

        assert_eq!(cache.frame_count(), 3);
    }

    #[test]
    fn test_cleanup_removes_files() {
        let temp_dir = std::env::temp_dir().join("lumos_cleanup_test");
        std::fs::create_dir_all(&temp_dir).unwrap();

        // Create fake cache files (3 channels for RGB)
        let paths = vec![
            temp_dir.join("frame0_c0.bin"),
            temp_dir.join("frame0_c1.bin"),
            temp_dir.join("frame0_c2.bin"),
        ];
        for path in &paths {
            std::fs::write(path, b"test data").unwrap();
            assert!(path.exists());
        }

        let cache = ImageCache {
            storage: Storage::DiskBacked {
                frames: vec![CachedFrame {
                    mmaps: vec![], // Empty mmaps for test
                    paths: paths.clone(),
                }],
                cache_dir: temp_dir.clone(),
            },
            dimensions: ImageDimensions::new(2, 2, 3),
            metadata: AstroImageMetadata::default(),
            config: CacheConfig::default(),
            progress: ProgressCallback::default(),
        };

        cache.cleanup();

        // Files should be removed
        for path in &paths {
            assert!(!path.exists(), "File should be deleted: {:?}", path);
        }
    }

    #[test]
    fn test_read_channel_chunk_in_memory() {
        let dims = ImageDimensions::new(4, 3, 1);
        // Pixels 0-11 in row-major order
        let pixels: Vec<f32> = (0..12).map(|i| i as f32).collect();
        let images = vec![AstroImage::from_pixels(dims, pixels)];

        let cache = ImageCache {
            storage: Storage::InMemory(images),
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
    }
}
