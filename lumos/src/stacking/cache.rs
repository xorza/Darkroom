//! Binary cache for decoded images with memory-mapped access.
//!
//! Cache format per file:
//! - Header: CacheHeader struct (width, height, channels as u32)
//! - Data: f32 pixels in row-major order (width * height * channels * 4 bytes)

use std::fs::File;
use std::io::Write;
use std::mem::size_of;
use std::path::{Path, PathBuf};

use memmap2::Mmap;

use crate::astro_image::{AstroImage, AstroImageMetadata, ImageDimensions};
use crate::stacking::FrameType;

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

/// Memory-mapped cache of decoded images.
#[derive(Debug)]
pub struct ImageCache {
    /// Memory-mapped cache files.
    mmaps: Vec<Mmap>,
    /// Paths to cache files (for cleanup).
    cache_paths: Vec<PathBuf>,
    /// Image dimensions (same for all frames).
    dimensions: ImageDimensions,
    /// Metadata from first frame.
    metadata: AstroImageMetadata,
    /// Cache directory.
    cache_dir: PathBuf,
}

impl ImageCache {
    /// Create cache from image paths, decoding each to binary format.
    pub fn from_paths<P: AsRef<Path>>(
        paths: &[P],
        cache_dir: &Path,
        frame_type: FrameType,
    ) -> Self {
        assert!(!paths.is_empty(), "No paths provided");

        let mut mmaps = Vec::with_capacity(paths.len());
        let mut cache_paths = Vec::with_capacity(paths.len());
        let mut dimensions: Option<ImageDimensions> = None;
        let mut metadata: Option<AstroImageMetadata> = None;

        for (i, path) in paths.iter().enumerate() {
            let image = AstroImage::from_file(path).expect("Failed to load image");

            if let Some(dims) = dimensions {
                assert!(
                    image.dimensions == dims,
                    "{} frame {} has different dimensions: {:?} vs {:?}",
                    frame_type,
                    i,
                    image.dimensions,
                    dims
                );
            } else {
                dimensions = Some(image.dimensions);
                metadata = Some(image.metadata.clone());
            }

            // Write to cache file
            let cache_path = cache_dir.join(format!("frame_{:04}.bin", i));
            write_cache_file(&cache_path, &image);

            // Memory-map the cache file
            let file = File::open(&cache_path).expect("Failed to open cache file");
            let mmap = unsafe { Mmap::map(&file).expect("Failed to mmap cache file") };

            mmaps.push(mmap);
            cache_paths.push(cache_path);
        }

        Self {
            mmaps,
            cache_paths,
            dimensions: dimensions.unwrap(),
            metadata: metadata.unwrap(),
            cache_dir: cache_dir.to_path_buf(),
        }
    }

    /// Get image dimensions.
    pub fn dimensions(&self) -> ImageDimensions {
        self.dimensions
    }

    /// Get metadata from first frame.
    pub fn metadata(&self) -> &AstroImageMetadata {
        &self.metadata
    }

    /// Get number of cached frames.
    pub fn frame_count(&self) -> usize {
        self.mmaps.len()
    }

    /// Read a horizontal chunk (rows start_row..end_row) from a cached frame.
    /// Returns a slice directly from the memory-mapped file (zero-copy).
    pub fn read_chunk(&self, frame_idx: usize, start_row: usize, end_row: usize) -> &[f32] {
        let mmap = &self.mmaps[frame_idx];
        let width = self.dimensions.width;
        let channels = self.dimensions.channels;

        let row_size = width * channels;
        let start_offset = size_of::<CacheHeader>() + start_row * row_size * 4;
        let end_offset = size_of::<CacheHeader>() + end_row * row_size * 4;

        let bytes = &mmap[start_offset..end_offset];
        bytemuck::cast_slice(bytes)
    }

    /// Remove all cache files.
    pub fn cleanup(&self) {
        for path in &self.cache_paths {
            let _ = std::fs::remove_file(path);
        }
        // Try to remove cache dir if empty
        let _ = std::fs::remove_dir(&self.cache_dir);
    }
}

/// Write image to binary cache file.
fn write_cache_file(path: &Path, image: &AstroImage) {
    let mut file = File::create(path).expect("Failed to create cache file");

    // Write header
    let header = CacheHeader {
        width: image.dimensions.width as u32,
        height: image.dimensions.height as u32,
        channels: image.dimensions.channels as u32,
    };
    file.write_all(bytemuck::bytes_of(&header)).unwrap();

    // Write pixel data
    let bytes: &[u8] = bytemuck::cast_slice(&image.pixels);
    file.write_all(bytes).unwrap();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cache_roundtrip() {
        let temp_dir = std::env::temp_dir().join("lumos_cache_test");
        std::fs::create_dir_all(&temp_dir).unwrap();

        // Create test image
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

        // Write to cache
        let cache_path = temp_dir.join("test_frame.bin");
        write_cache_file(&cache_path, &image);

        // Memory-map and read back
        let file = File::open(&cache_path).unwrap();
        let mmap = unsafe { Mmap::map(&file).unwrap() };

        // Verify header
        let header: &CacheHeader = bytemuck::from_bytes(&mmap[..size_of::<CacheHeader>()]);

        assert_eq!(header.width, 4);
        assert_eq!(header.height, 3);
        assert_eq!(header.channels, 3);

        // Verify data
        let data_bytes = &mmap[size_of::<CacheHeader>()..];
        let read_pixels: &[f32] = bytemuck::cast_slice(data_bytes);

        assert_eq!(read_pixels.len(), pixels.len());
        for (a, b) in read_pixels.iter().zip(pixels.iter()) {
            assert!((a - b).abs() < f32::EPSILON);
        }

        // Cleanup
        std::fs::remove_file(&cache_path).unwrap();
        let _ = std::fs::remove_dir(&temp_dir);
    }

    #[test]
    fn test_read_chunk() {
        let temp_dir = std::env::temp_dir().join("lumos_chunk_test");
        std::fs::create_dir_all(&temp_dir).unwrap();

        // Create test image: 4x3x3 (width x height x channels)
        let dims = ImageDimensions {
            width: 4,
            height: 3,
            channels: 3,
        };
        // Row 0: 0-11, Row 1: 12-23, Row 2: 24-35
        let pixels: Vec<f32> = (0..36).map(|i| i as f32).collect();
        let image = AstroImage {
            metadata: AstroImageMetadata::default(),
            pixels,
            dimensions: dims,
        };

        let cache_path = temp_dir.join("chunk_frame.bin");
        write_cache_file(&cache_path, &image);

        let file = File::open(&cache_path).unwrap();
        let mmap = unsafe { Mmap::map(&file).unwrap() };

        // Manually read chunk for row 1 only
        let width = 4;
        let channels = 3;
        let row_size = width * channels; // 12
        let start_offset = size_of::<CacheHeader>() + row_size * 4; // row 1
        let end_offset = size_of::<CacheHeader>() + 2 * row_size * 4; // up to row 2

        let bytes = &mmap[start_offset..end_offset];
        let chunk: &[f32] = bytemuck::cast_slice(bytes);

        // Row 1 should be 12..24
        let expected: Vec<f32> = (12..24).map(|i| i as f32).collect();
        assert_eq!(chunk.len(), expected.len());
        for (a, b) in chunk.iter().zip(expected.iter()) {
            assert!((a - b).abs() < f32::EPSILON);
        }

        // Cleanup
        std::fs::remove_file(&cache_path).unwrap();
        let _ = std::fs::remove_dir(&temp_dir);
    }
}
