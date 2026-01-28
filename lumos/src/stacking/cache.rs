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

/// Storage mode for image data.
#[derive(Debug)]
enum Storage {
    /// All images fit in memory - stores AstroImage directly for planar channel access.
    InMemory(Vec<AstroImage>),
    /// Images stored on disk with memory-mapped access.
    /// Each frame has one file per channel for efficient planar access.
    DiskBacked {
        /// mmaps[frame_idx][channel_idx] - memory-mapped channel data
        mmaps: Vec<Vec<Mmap>>,
        /// cache_paths[frame_idx][channel_idx] - paths to channel cache files
        cache_paths: Vec<Vec<PathBuf>>,
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
        let mut all_mmaps: Vec<Vec<Mmap>> = Vec::with_capacity(paths.len());
        let mut all_cache_paths: Vec<Vec<PathBuf>> = Vec::with_capacity(paths.len());

        // Write first image (or reuse existing cache)
        let first_path = paths[0].as_ref();
        let base_filename = cache_filename_for_path(first_path);
        tracing::info!(
            source = %first_path.display(),
            cache_base = %base_filename,
            "Mapping source to cache files"
        );

        let (frame_mmaps, frame_paths) =
            cache_image_channels(cache_dir, &base_filename, &first_image, dimensions)?;
        all_mmaps.push(frame_mmaps);
        all_cache_paths.push(frame_paths);
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
                all_mmaps.push(frame_mmaps);
                all_cache_paths.push(frame_paths);
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

                let (frame_mmaps, frame_paths) =
                    cache_image_channels(cache_dir, &base_filename, &image, dimensions)?;
                all_mmaps.push(frame_mmaps);
                all_cache_paths.push(frame_paths);
            }

            report_progress(progress, i + 1, paths.len(), StackingStage::Loading);
        }

        tracing::info!(
            "Cached {} frames ({} channels each) to disk at {:?}",
            all_mmaps.len(),
            channels,
            cache_dir
        );

        Ok(Storage::DiskBacked {
            mmaps: all_mmaps,
            cache_paths: all_cache_paths,
            cache_dir: cache_dir.to_path_buf(),
        })
    }

    /// Get number of cached frames.
    pub fn frame_count(&self) -> usize {
        match &self.storage {
            Storage::InMemory(images) => images.len(),
            Storage::DiskBacked { mmaps, .. } => mmaps.len(),
        }
    }

    /// Remove all cache files (only applies to disk-backed storage).
    pub fn cleanup(&self) {
        if let Storage::DiskBacked {
            cache_paths,
            cache_dir,
            ..
        } = &self.storage
        {
            for frame_paths in cache_paths {
                for path in frame_paths {
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
        for channel in 0..channels {
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

                let output_slice =
                    &mut output_channels[channel][start_row * width..][..pixels_in_chunk];

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
        for channel in 0..channels {
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

                let output_slice =
                    &mut output_channels[channel][start_row * width..][..pixels_in_chunk];

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
            Storage::DiskBacked { mmaps, .. } => {
                let mmap = &mmaps[frame_idx][channel];
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

/// Cache all channels of an image to separate files and return mmaps.
fn cache_image_channels(
    cache_dir: &Path,
    base_filename: &str,
    image: &AstroImage,
    dimensions: ImageDimensions,
) -> Result<(Vec<Mmap>, Vec<PathBuf>), Error> {
    let channels = dimensions.channels;
    let mut frame_mmaps = Vec::with_capacity(channels);
    let mut frame_paths = Vec::with_capacity(channels);

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

        frame_mmaps.push(mmap);
        frame_paths.push(channel_path);
    }

    Ok((frame_mmaps, frame_paths))
}

#[cfg(test)]
mod tests {
    use super::*;

    // ========== Storage Type Selection Tests ==========

    #[test]
    fn test_in_memory_when_images_fit() {
        // 10 images of 1000x1000x3 = 10 * 3M pixels * 4 bytes = 120MB
        // With 1GB available and 75% usable = 750MB
        // 120MB < 750MB, so should use in-memory
        let pixel_count = 1000 * 1000 * 3;
        let frame_count = 10;
        let available = 1024 * 1024 * 1024u64; // 1GB

        assert!(ImageCache::fits_in_memory(
            pixel_count,
            frame_count,
            available
        ));
    }

    #[test]
    fn test_disk_backed_when_images_exceed_memory() {
        // 100 images of 6000x4000x3 = 100 * 72M pixels * 4 bytes = 28.8GB
        // With 16GB available and 75% usable = 12GB
        // 28.8GB > 12GB, so should use disk
        let pixel_count = 6000 * 4000 * 3;
        let frame_count = 100;
        let available = 16 * 1024 * 1024 * 1024u64; // 16GB

        assert!(!ImageCache::fits_in_memory(
            pixel_count,
            frame_count,
            available
        ));
    }

    #[test]
    fn test_in_memory_at_boundary() {
        // Set up exactly at 75% boundary
        // pixel_count * frame_count * 4 = available * 0.75
        let pixel_count = 1000 * 1000; // 1M pixels
        let frame_count = 10;
        let bytes_needed = (pixel_count * frame_count * 4) as u64; // 40MB
        // usable = available * 75/100 = bytes_needed
        // We need: bytes_needed <= available * 75 / 100
        // Rearranging: available >= bytes_needed * 100 / 75
        // Integer division truncates, so we need to ensure usable >= bytes_needed
        // available * 75 / 100 >= bytes_needed
        // To get exactly at boundary with integer math, find smallest available where usable == bytes_needed
        let available = (bytes_needed * 100).div_ceil(75); // Round up to ensure usable >= bytes_needed

        // At exact boundary, should fit
        assert!(ImageCache::fits_in_memory(
            pixel_count,
            frame_count,
            available
        ));

        // With less usable memory, should not fit
        // Reduce available so usable < bytes_needed
        let reduced_available = (bytes_needed * 100) / 75 - 1;
        assert!(!ImageCache::fits_in_memory(
            pixel_count,
            frame_count,
            reduced_available
        ));
    }

    #[test]
    fn test_in_memory_small_stack() {
        // Small stack: 5 images of 2000x1500x3 = 5 * 9M pixels * 4 = 180MB
        // With 512MB available (75% = 384MB), should fit
        let pixel_count = 2000 * 1500 * 3;
        let frame_count = 5;
        let available = 512 * 1024 * 1024u64;

        assert!(ImageCache::fits_in_memory(
            pixel_count,
            frame_count,
            available
        ));
    }

    #[test]
    fn test_disk_backed_low_memory() {
        // Same small stack but only 128MB available (75% = 96MB)
        // 180MB > 96MB, should use disk
        let pixel_count = 2000 * 1500 * 3;
        let frame_count = 5;
        let available = 128 * 1024 * 1024u64;

        assert!(!ImageCache::fits_in_memory(
            pixel_count,
            frame_count,
            available
        ));
    }

    #[test]
    fn test_in_memory_single_frame() {
        // Single frame should almost always fit
        let pixel_count = 8000 * 6000 * 3; // 144M pixels * 4 = 576MB
        let frame_count = 1;
        let available = 1024 * 1024 * 1024u64; // 1GB (75% = 768MB)

        assert!(ImageCache::fits_in_memory(
            pixel_count,
            frame_count,
            available
        ));
    }

    #[test]
    fn test_storage_decision_grayscale_vs_rgb() {
        // Grayscale: 6000x4000x1 * 20 frames = 480M pixels * 4 = 1.92GB
        // RGB: 6000x4000x3 * 20 frames = 1.44B pixels * 4 = 5.76GB
        // With 4GB available (75% = 3GB):
        // - Grayscale should fit
        // - RGB should not fit
        let available = 4 * 1024 * 1024 * 1024u64;
        let frame_count = 20;

        let grayscale_pixels = 6000 * 4000; // 1 channel
        let rgb_pixels = 6000 * 4000 * 3;

        assert!(ImageCache::fits_in_memory(
            grayscale_pixels,
            frame_count,
            available
        ));
        assert!(!ImageCache::fits_in_memory(
            rgb_pixels,
            frame_count,
            available
        ));
    }

    // ========== Cache File Tests ==========

    #[test]
    fn test_channel_cache_roundtrip() {
        let temp_dir = std::env::temp_dir().join("lumos_channel_cache_test");
        std::fs::create_dir_all(&temp_dir).unwrap();

        let dims = ImageDimensions::new(4, 3, 3);
        // Interleaved RGB pixels
        let pixels: Vec<f32> = (0..36).map(|i| i as f32).collect();
        let image = AstroImage::from_pixels(dims, pixels);

        let base_filename = "test_frame.bin";
        let (mmaps, paths) = cache_image_channels(&temp_dir, base_filename, &image, dims).unwrap();

        // Should create 3 channel files
        assert_eq!(mmaps.len(), 3);
        assert_eq!(paths.len(), 3);

        // Verify each channel file contains the correct data
        for c in 0..3 {
            let mmap = &mmaps[c];
            let read_channel: &[f32] = bytemuck::cast_slice(&mmap[..]);
            let expected_channel = image.channel(c);

            assert_eq!(read_channel.len(), expected_channel.len());
            for (a, b) in read_channel.iter().zip(expected_channel.iter()) {
                assert!((a - b).abs() < f32::EPSILON);
            }
        }

        // Cleanup
        for path in paths {
            let _ = std::fs::remove_file(path);
        }
        let _ = std::fs::remove_dir(&temp_dir);
    }

    #[test]
    fn test_read_channel_chunk() {
        let temp_dir = std::env::temp_dir().join("lumos_channel_chunk_test");
        std::fs::create_dir_all(&temp_dir).unwrap();

        let dims = ImageDimensions::new(4, 3, 1);
        // Grayscale: 12 pixels
        let pixels: Vec<f32> = (0..12).map(|i| i as f32).collect();
        let image = AstroImage::from_pixels(dims, pixels);

        // Write channel 0 to cache
        let channel_path = temp_dir.join("chunk_c0.bin");
        write_channel_cache_file(&channel_path, image.channel(0)).unwrap();

        let file = File::open(&channel_path).unwrap();
        let mmap = unsafe { Mmap::map(&file).unwrap() };

        // Read row 1 (pixels 4-7)
        let width = 4;
        let start_offset = 1 * width * size_of::<f32>();
        let end_offset = 2 * width * size_of::<f32>();

        let bytes = &mmap[start_offset..end_offset];
        let chunk: &[f32] = bytemuck::cast_slice(bytes);

        let expected: Vec<f32> = (4..8).map(|i| i as f32).collect();
        assert_eq!(chunk.len(), expected.len());
        for (a, b) in chunk.iter().zip(expected.iter()) {
            assert!((a - b).abs() < f32::EPSILON);
        }

        std::fs::remove_file(&channel_path).unwrap();
        let _ = std::fs::remove_dir(&temp_dir);
    }

    // ========== Error Path Tests ==========

    #[test]
    fn test_from_paths_empty_returns_no_paths_error() {
        let paths: Vec<PathBuf> = vec![];
        let config = CacheConfig::default();
        let result = ImageCache::from_paths(
            &paths,
            &config,
            FrameType::Dark,
            ProgressCallback::default(),
        );

        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(matches!(err, Error::NoPaths));
    }

    #[test]
    fn test_from_paths_nonexistent_file_returns_image_load_error() {
        let paths = vec![PathBuf::from("/nonexistent/path/to/image.fits")];
        let config = CacheConfig::default();
        let result = ImageCache::from_paths(
            &paths,
            &config,
            FrameType::Dark,
            ProgressCallback::default(),
        );

        assert!(result.is_err());
        let err = result.unwrap_err();
        match err {
            Error::ImageLoad { path, .. } => {
                assert!(path.to_string_lossy().contains("nonexistent"));
            }
            _ => panic!("Expected ImageLoad error, got {:?}", err),
        }
    }

    #[test]
    fn test_channel_cache_reuse_with_matching_dimensions() {
        let temp_dir = std::env::temp_dir().join("lumos_channel_reuse_test");
        std::fs::create_dir_all(&temp_dir).unwrap();

        let dims = ImageDimensions::new(4, 3, 1);
        let pixels: Vec<f32> = (0..12).map(|i| i as f32).collect();
        let image = AstroImage::from_pixels(dims, pixels);

        let cache_path = temp_dir.join("reuse_c0.bin");
        write_channel_cache_file(&cache_path, image.channel(0)).unwrap();

        // File should be reusable with same dimensions
        assert!(try_reuse_channel_cache_file(&cache_path, dims));

        // File should not be reusable with different dimensions (different pixel count)
        let different_dims = ImageDimensions::new(8, 3, 1);
        assert!(!try_reuse_channel_cache_file(&cache_path, different_dims));

        std::fs::remove_file(&cache_path).unwrap();
        let _ = std::fs::remove_dir(&temp_dir);
    }

    #[test]
    fn test_channel_cache_reuse_nonexistent_file() {
        let dims = ImageDimensions::new(4, 3, 1);
        // Nonexistent file should not be reusable
        assert!(!try_reuse_channel_cache_file(
            Path::new("/nonexistent/file.bin"),
            dims
        ));
    }

    #[test]
    fn test_channel_cache_reuse_wrong_size_file() {
        let temp_dir = std::env::temp_dir().join("lumos_channel_wrong_size_test");
        std::fs::create_dir_all(&temp_dir).unwrap();

        let cache_path = temp_dir.join("wrong_size.bin");
        // Write a file with wrong size
        std::fs::write(&cache_path, b"too short").unwrap();

        let dims = ImageDimensions::new(4, 3, 1);
        assert!(!try_reuse_channel_cache_file(&cache_path, dims));

        std::fs::remove_file(&cache_path).unwrap();
        let _ = std::fs::remove_dir(&temp_dir);
    }

    #[test]
    fn test_write_channel_cache_file_creates_valid_file() {
        let temp_dir = std::env::temp_dir().join("lumos_channel_write_test");
        std::fs::create_dir_all(&temp_dir).unwrap();

        let dims = ImageDimensions::new(2, 2, 1);
        let pixels: Vec<f32> = vec![1.0, 2.0, 3.0, 4.0];
        let image = AstroImage::from_pixels(dims, pixels);

        let cache_path = temp_dir.join("valid_write_c0.bin");
        let result = write_channel_cache_file(&cache_path, image.channel(0));
        assert!(result.is_ok());

        // Verify file exists and has correct size (raw f32 data, no header)
        let metadata = std::fs::metadata(&cache_path).unwrap();
        let expected_size = 4 * size_of::<f32>();
        assert_eq!(metadata.len(), expected_size as u64);

        std::fs::remove_file(&cache_path).unwrap();
        let _ = std::fs::remove_dir(&temp_dir);
    }

    #[test]
    fn test_channel_cache_filename() {
        assert_eq!(channel_cache_filename("abc123.bin", 0), "abc123_c0.bin");
        assert_eq!(channel_cache_filename("abc123.bin", 1), "abc123_c1.bin");
        assert_eq!(channel_cache_filename("abc123.bin", 2), "abc123_c2.bin");
    }
}
