//! Image cache for stacking operations.
//!
//! Supports two modes:
//! - In-memory: When all images fit in available RAM (75%), keeps them in memory
//! - Disk-based: Uses memory-mapped binary files for larger datasets
//!
//! Cache format per file (disk mode):
//! - Header: CacheHeader struct (width, height, channels as u32)
//! - Data: f32 pixels in row-major order (width * height * channels * 4 bytes)

use std::fs::File;
use std::io::{self, BufWriter, Write};
use std::mem::size_of;
use std::path::{Path, PathBuf};

use memmap2::Mmap;
use rayon::prelude::*;

use crate::astro_image::{AstroImage, AstroImageMetadata, ImageDimensions};
use crate::stacking::FrameType;
use crate::stacking::cache_config::{
    CacheConfig, CacheStage, MEMORY_PERCENT, compute_optimal_chunk_rows_with_memory,
};
use crate::stacking::error::StackError;

/// Header for cached image files.
#[derive(Debug, Clone, Copy)]
#[repr(C)]
struct CacheHeader {
    width: u32,
    height: u32,
    channels: u32,
}

unsafe impl bytemuck::Pod for CacheHeader {}
unsafe impl bytemuck::Zeroable for CacheHeader {}

/// Storage mode for image data.
#[derive(Debug)]
enum Storage {
    /// All images fit in memory - no disk I/O needed.
    InMemory(Vec<AstroImage>),
    /// Images stored on disk with memory-mapped access.
    DiskBacked {
        mmaps: Vec<Mmap>,
        cache_paths: Vec<PathBuf>,
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
}

impl ImageCache {
    /// Check if images would fit in memory given available memory.
    ///
    /// Returns true if total image size fits within usable memory (75% of available).
    fn fits_in_memory(pixel_count: usize, frame_count: usize, available_memory: u64) -> bool {
        let bytes_per_image = pixel_count * size_of::<f32>();
        let total_bytes_needed = bytes_per_image * frame_count;
        let usable_memory = available_memory * MEMORY_PERCENT / 100;
        (total_bytes_needed as u64) <= usable_memory
    }

    /// Create cache from image paths.
    ///
    /// Automatically chooses storage mode based on available memory:
    /// - If all images fit in 75% of available RAM, keeps them in memory
    /// - Otherwise, writes to disk cache and uses memory-mapped access
    pub fn from_paths<P: AsRef<Path>>(
        paths: &[P],
        config: &CacheConfig,
        frame_type: FrameType,
    ) -> Result<Self, StackError> {
        if paths.is_empty() {
            return Err(StackError::NoPaths);
        }

        // Report initial progress
        config.report_progress(0, paths.len(), CacheStage::Loading);

        // Load first image to get dimensions
        let first_path = paths[0].as_ref();
        let first_image = AstroImage::from_file(first_path).map_err(|e| StackError::ImageLoad {
            path: first_path.to_path_buf(),
            source: io::Error::other(e.to_string()),
        })?;
        let dimensions = first_image.dimensions;
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
            Self::load_in_memory(paths, config, frame_type, dimensions, first_image)?
        } else {
            Self::load_to_disk(paths, config, frame_type, dimensions, first_image)?
        };

        Ok(Self {
            storage,
            dimensions,
            metadata,
            config: config.clone(),
        })
    }

    /// Load all images into memory.
    fn load_in_memory<P: AsRef<Path>>(
        paths: &[P],
        config: &CacheConfig,
        frame_type: FrameType,
        dimensions: ImageDimensions,
        first_image: AstroImage,
    ) -> Result<Storage, StackError> {
        let mut images = Vec::with_capacity(paths.len());
        images.push(first_image);
        config.report_progress(1, paths.len(), CacheStage::Loading);

        for (i, path) in paths.iter().enumerate().skip(1) {
            let path_ref = path.as_ref();
            let image = AstroImage::from_file(path_ref).map_err(|e| StackError::ImageLoad {
                path: path_ref.to_path_buf(),
                source: io::Error::other(e.to_string()),
            })?;

            if image.dimensions != dimensions {
                return Err(StackError::DimensionMismatch {
                    frame_type,
                    index: i,
                    expected: dimensions,
                    actual: image.dimensions,
                });
            }

            images.push(image);
            config.report_progress(i + 1, paths.len(), CacheStage::Loading);
        }

        tracing::info!("Loaded {} frames into memory", images.len());
        Ok(Storage::InMemory(images))
    }

    /// Load images to disk cache with memory-mapped access.
    fn load_to_disk<P: AsRef<Path>>(
        paths: &[P],
        config: &CacheConfig,
        frame_type: FrameType,
        dimensions: ImageDimensions,
        first_image: AstroImage,
    ) -> Result<Storage, StackError> {
        let cache_dir = &config.cache_dir;
        std::fs::create_dir_all(cache_dir).map_err(|e| StackError::CreateCacheDir {
            path: cache_dir.to_path_buf(),
            source: e,
        })?;

        let mut mmaps = Vec::with_capacity(paths.len());
        let mut cache_paths = Vec::with_capacity(paths.len());

        // Write first image (or reuse existing cache)
        let first_cache_path = cache_dir.join("frame_0000.bin");
        if !try_reuse_cache_file(&first_cache_path, dimensions) {
            write_cache_file(&first_cache_path, &first_image)?;
        }
        let file = File::open(&first_cache_path).map_err(|e| StackError::OpenCacheFile {
            path: first_cache_path.clone(),
            source: e,
        })?;
        let mmap = unsafe {
            Mmap::map(&file).map_err(|e| StackError::MmapCacheFile {
                path: first_cache_path.clone(),
                source: e,
            })?
        };
        mmaps.push(mmap);
        cache_paths.push(first_cache_path);
        config.report_progress(1, paths.len(), CacheStage::Loading);

        // Process remaining images
        for (i, path) in paths.iter().enumerate().skip(1) {
            let cache_path = cache_dir.join(format!("frame_{:04}.bin", i));

            // Try to reuse existing cache file if dimensions match
            if !try_reuse_cache_file(&cache_path, dimensions) {
                let path_ref = path.as_ref();
                let image = AstroImage::from_file(path_ref).map_err(|e| StackError::ImageLoad {
                    path: path_ref.to_path_buf(),
                    source: io::Error::other(e.to_string()),
                })?;

                if image.dimensions != dimensions {
                    return Err(StackError::DimensionMismatch {
                        frame_type,
                        index: i,
                        expected: dimensions,
                        actual: image.dimensions,
                    });
                }

                write_cache_file(&cache_path, &image)?;
            }

            let file = File::open(&cache_path).map_err(|e| StackError::OpenCacheFile {
                path: cache_path.clone(),
                source: e,
            })?;
            let mmap = unsafe {
                Mmap::map(&file).map_err(|e| StackError::MmapCacheFile {
                    path: cache_path.clone(),
                    source: e,
                })?
            };

            mmaps.push(mmap);
            cache_paths.push(cache_path);
            config.report_progress(i + 1, paths.len(), CacheStage::Loading);
        }

        tracing::info!("Cached {} frames to disk at {:?}", mmaps.len(), cache_dir);

        Ok(Storage::DiskBacked {
            mmaps,
            cache_paths,
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
            for path in cache_paths {
                let _ = std::fs::remove_file(path);
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
    pub fn process_chunked<F>(&self, combine: F) -> AstroImage
    where
        F: Fn(&mut [f32]) -> f32 + Sync,
    {
        let dims = self.dimensions;
        let frame_count = self.frame_count();
        let width = dims.width;
        let height = dims.height;
        let channels = dims.channels;
        let available_memory = self.config.get_available_memory();

        // For in-memory, process entire image; for disk, use adaptive chunking
        let chunk_rows = match &self.storage {
            Storage::InMemory(_) => height, // Process all at once
            Storage::DiskBacked { .. } => compute_optimal_chunk_rows_with_memory(
                width,
                channels,
                frame_count,
                available_memory,
            ),
        };

        let mut output_pixels = vec![0.0f32; dims.pixel_count()];
        let num_chunks = height.div_ceil(chunk_rows);

        self.config
            .report_progress(0, num_chunks, CacheStage::Processing);

        for chunk_idx in 0..num_chunks {
            let start_row = chunk_idx * chunk_rows;
            let end_row = (start_row + chunk_rows).min(height);
            let rows_in_chunk = end_row - start_row;
            let pixels_in_chunk = rows_in_chunk * width * channels;

            // Get slices from all frames
            let chunks: Vec<&[f32]> = (0..frame_count)
                .map(|frame_idx| self.read_chunk(frame_idx, start_row, end_row))
                .collect();

            let output_slice =
                &mut output_pixels[start_row * width * channels..][..pixels_in_chunk];

            output_slice
                .par_chunks_mut(width * channels)
                .enumerate()
                .for_each(|(row_in_chunk, row_output)| {
                    // One buffer per thread, reused for all pixels in row
                    let mut values = vec![0.0f32; frame_count];
                    let row_offset = row_in_chunk * width * channels;

                    for (pixel_in_row, out) in row_output.iter_mut().enumerate() {
                        let pixel_idx = row_offset + pixel_in_row;
                        // Gather values from all frames
                        for (frame_idx, chunk) in chunks.iter().enumerate() {
                            values[frame_idx] = chunk[pixel_idx];
                        }
                        *out = combine(&mut values);
                    }
                });

            self.config
                .report_progress(chunk_idx + 1, num_chunks, CacheStage::Processing);
        }

        AstroImage {
            metadata: self.metadata.clone(),
            pixels: output_pixels,
            dimensions: dims,
        }
    }

    /// Read a horizontal chunk (rows start_row..end_row) from a frame.
    fn read_chunk(&self, frame_idx: usize, start_row: usize, end_row: usize) -> &[f32] {
        let width = self.dimensions.width;
        let channels = self.dimensions.channels;
        let row_size = width * channels;
        let start_pixel = start_row * row_size;
        let end_pixel = end_row * row_size;

        match &self.storage {
            Storage::InMemory(images) => &images[frame_idx].pixels[start_pixel..end_pixel],
            Storage::DiskBacked { mmaps, .. } => {
                let mmap = &mmaps[frame_idx];
                let start_offset = size_of::<CacheHeader>() + start_pixel * 4;
                let end_offset = size_of::<CacheHeader>() + end_pixel * 4;
                let bytes = &mmap[start_offset..end_offset];
                bytemuck::cast_slice(bytes)
            }
        }
    }
}

/// Try to reuse an existing cache file if it exists and has matching dimensions.
///
/// Returns true if the file can be reused, false if it needs to be rewritten.
fn try_reuse_cache_file(path: &Path, expected_dims: ImageDimensions) -> bool {
    let Ok(file) = File::open(path) else {
        return false;
    };

    // Check file size matches expected
    let expected_size = size_of::<CacheHeader>() + expected_dims.pixel_count() * size_of::<f32>();
    let Ok(metadata) = file.metadata() else {
        return false;
    };
    if metadata.len() != expected_size as u64 {
        return false;
    }

    // Check header dimensions match
    let Ok(mmap) = (unsafe { Mmap::map(&file) }) else {
        return false;
    };
    if mmap.len() < size_of::<CacheHeader>() {
        return false;
    }

    let header: &CacheHeader = bytemuck::from_bytes(&mmap[..size_of::<CacheHeader>()]);
    let matches = header.width == expected_dims.width as u32
        && header.height == expected_dims.height as u32
        && header.channels == expected_dims.channels as u32;

    if matches {
        tracing::debug!("Reusing existing cache file: {:?}", path);
    }
    matches
}

/// Write image to binary cache file.
fn write_cache_file(path: &Path, image: &AstroImage) -> Result<(), StackError> {
    let file = File::create(path).map_err(|e| StackError::CreateCacheFile {
        path: path.to_path_buf(),
        source: e,
    })?;
    let mut writer = BufWriter::new(file);

    let header = CacheHeader {
        width: image.dimensions.width as u32,
        height: image.dimensions.height as u32,
        channels: image.dimensions.channels as u32,
    };
    writer
        .write_all(bytemuck::bytes_of(&header))
        .map_err(|e| StackError::WriteCacheFile {
            path: path.to_path_buf(),
            source: e,
        })?;

    let bytes: &[u8] = bytemuck::cast_slice(&image.pixels);
    writer
        .write_all(bytes)
        .map_err(|e| StackError::WriteCacheFile {
            path: path.to_path_buf(),
            source: e,
        })?;
    writer.flush().map_err(|e| StackError::WriteCacheFile {
        path: path.to_path_buf(),
        source: e,
    })?;

    Ok(())
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
    fn test_cache_roundtrip() {
        let temp_dir = std::env::temp_dir().join("lumos_cache_test");
        std::fs::create_dir_all(&temp_dir).unwrap();

        let dims = ImageDimensions {
            width: 4,
            height: 3,
            channels: 3,
        };
        let pixels: Vec<f32> = (0..36).map(|i| i as f32).collect();
        let image = AstroImage {
            metadata: AstroImageMetadata::default(),
            pixels: pixels.clone(),
            dimensions: dims,
        };

        let cache_path = temp_dir.join("test_frame.bin");
        write_cache_file(&cache_path, &image).unwrap();

        let file = File::open(&cache_path).unwrap();
        let mmap = unsafe { Mmap::map(&file).unwrap() };

        let header: &CacheHeader = bytemuck::from_bytes(&mmap[..size_of::<CacheHeader>()]);
        assert_eq!(header.width, 4);
        assert_eq!(header.height, 3);
        assert_eq!(header.channels, 3);

        let data_bytes = &mmap[size_of::<CacheHeader>()..];
        let read_pixels: &[f32] = bytemuck::cast_slice(data_bytes);

        assert_eq!(read_pixels.len(), pixels.len());
        for (a, b) in read_pixels.iter().zip(pixels.iter()) {
            assert!((a - b).abs() < f32::EPSILON);
        }

        std::fs::remove_file(&cache_path).unwrap();
        let _ = std::fs::remove_dir(&temp_dir);
    }

    #[test]
    fn test_read_chunk() {
        let temp_dir = std::env::temp_dir().join("lumos_chunk_test");
        std::fs::create_dir_all(&temp_dir).unwrap();

        let dims = ImageDimensions {
            width: 4,
            height: 3,
            channels: 3,
        };
        let pixels: Vec<f32> = (0..36).map(|i| i as f32).collect();
        let image = AstroImage {
            metadata: AstroImageMetadata::default(),
            pixels,
            dimensions: dims,
        };

        let cache_path = temp_dir.join("chunk_frame.bin");
        write_cache_file(&cache_path, &image).unwrap();

        let file = File::open(&cache_path).unwrap();
        let mmap = unsafe { Mmap::map(&file).unwrap() };

        let width = 4;
        let channels = 3;
        let row_size = width * channels;
        let start_offset = size_of::<CacheHeader>() + row_size * 4;
        let end_offset = size_of::<CacheHeader>() + 2 * row_size * 4;

        let bytes = &mmap[start_offset..end_offset];
        let chunk: &[f32] = bytemuck::cast_slice(bytes);

        let expected: Vec<f32> = (12..24).map(|i| i as f32).collect();
        assert_eq!(chunk.len(), expected.len());
        for (a, b) in chunk.iter().zip(expected.iter()) {
            assert!((a - b).abs() < f32::EPSILON);
        }

        std::fs::remove_file(&cache_path).unwrap();
        let _ = std::fs::remove_dir(&temp_dir);
    }
}
