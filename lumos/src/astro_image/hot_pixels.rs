//! Hot pixel detection and correction.
//!
//! Detects defective sensor pixels from master dark frames and corrects them
//! by replacing with the median of 8-connected neighbors.

use rayon::prelude::*;

use super::{AstroImage, ImageDimensions};

/// A mask of hot (defective) pixels detected from a master dark frame.
#[derive(Debug, Clone)]
pub struct HotPixelMap {
    /// Boolean mask where true = hot pixel
    mask: Vec<bool>,
    /// Image dimensions
    pub dimensions: ImageDimensions,
    /// Number of hot pixels detected
    pub count: usize,
}

impl HotPixelMap {
    /// Detect hot pixels from a master dark frame.
    ///
    /// Hot pixels are detected per-channel: each channel (R, G, B) is analyzed separately
    /// to compute its own median and MAD. A pixel is marked hot if ANY of its channels
    /// exceeds the threshold for that channel.
    ///
    /// Uses Median Absolute Deviation (MAD) for robust σ estimation that isn't affected
    /// by the outliers we're trying to detect.
    /// Typical threshold is 5.0σ.
    ///
    /// # Arguments
    /// * `master_dark` - The master dark frame to analyze
    /// * `sigma_threshold` - Number of standard deviations above median to flag as hot (typically 5.0)
    pub fn from_master_dark(master_dark: &AstroImage, sigma_threshold: f32) -> Self {
        assert!(sigma_threshold > 0.0, "Sigma threshold must be positive");

        let pixels = &master_dark.pixels;
        let channels = master_dark.dimensions.channels;
        let pixel_count = master_dark.dimensions.width * master_dark.dimensions.height;

        // Compute per-channel thresholds
        let channel_stats: Vec<ChannelStats> = (0..channels)
            .map(|c| compute_channel_stats(pixels, channels, c, sigma_threshold))
            .collect();

        tracing::debug!(
            "Hot pixel detection per-channel stats ({}x{}x{}):",
            master_dark.dimensions.width,
            master_dark.dimensions.height,
            channels
        );
        for (c, stats) in channel_stats.iter().enumerate() {
            tracing::debug!(
                "  Channel {}: median={:.6}, MAD={:.6}, sigma={:.6}, threshold={:.6}",
                c,
                stats.median,
                stats.mad,
                stats.sigma,
                stats.threshold
            );
        }

        let thresholds: Vec<f32> = channel_stats.iter().map(|s| s.threshold).collect();

        // Detect hot pixels - a pixel is hot if ANY channel exceeds its threshold
        // The mask is per-value (not per-pixel), so we check each channel value
        const BOOL_CHUNK_SIZE: usize = 16384;
        let mut mask = vec![false; pixels.len()];
        let _count: usize = mask
            .par_chunks_mut(BOOL_CHUNK_SIZE)
            .enumerate()
            .map(|(chunk_idx, chunk)| {
                let start = chunk_idx * BOOL_CHUNK_SIZE;
                let mut local_count = 0usize;
                for (i, val) in chunk.iter_mut().enumerate() {
                    let idx = start + i;
                    let channel = idx % channels;
                    let is_hot = pixels[idx] > thresholds[channel];
                    *val = is_hot;
                    local_count += is_hot as usize;
                }
                local_count
            })
            .sum();

        // For reporting, count unique pixels (not channel values)
        let pixel_hot_count = (0..pixel_count)
            .into_par_iter()
            .filter(|&p| {
                let base = p * channels;
                (0..channels).any(|c| mask[base + c])
            })
            .count();

        Self {
            mask,
            dimensions: master_dark.dimensions,
            count: pixel_hot_count,
        }
    }

    /// Check if a pixel at the given index is hot.
    #[inline]
    pub fn is_hot(&self, index: usize) -> bool {
        self.mask[index]
    }

    /// Check if a pixel at (x, y, channel) is hot.
    #[inline]
    pub fn is_hot_at(&self, x: usize, y: usize, channel: usize) -> bool {
        let idx = (y * self.dimensions.width + x) * self.dimensions.channels + channel;
        self.mask[idx]
    }

    /// Get the percentage of hot pixels (as percentage of total pixels, not channel values).
    pub fn percentage(&self) -> f32 {
        let pixel_count = self.dimensions.width * self.dimensions.height;
        100.0 * self.count as f32 / pixel_count as f32
    }
}

impl HotPixelMap {
    /// Correct hot pixels in an image by replacing them with median of 8-connected neighbors.
    ///
    /// # Arguments
    /// * `image` - The image to correct (modified in place)
    ///
    /// # Panics
    /// Panics if image dimensions don't match hot pixel map dimensions.
    pub fn correct(&self, image: &mut AstroImage) {
        assert!(
            image.dimensions == self.dimensions,
            "Image dimensions {:?} don't match hot pixel map {:?}",
            image.dimensions,
            self.dimensions
        );

        let width = image.dimensions.width;
        let height = image.dimensions.height;
        let channels = image.dimensions.channels;
        let row_stride = width * channels;

        // Collect corrections first (can't mutate while iterating)
        // Process rows in parallel - each row is independent
        let corrections: Vec<(usize, f32)> = (0..height)
            .into_par_iter()
            .fold(Vec::new, |mut local_corrections, y| {
                for x in 0..width {
                    for c in 0..channels {
                        let idx = y * row_stride + x * channels + c;
                        if self.is_hot(idx) {
                            let replacement = median_of_neighbors(image, x, y, c);
                            local_corrections.push((idx, replacement));
                        }
                    }
                }
                local_corrections
            })
            .reduce(Vec::new, |mut a, b| {
                a.extend(b);
                a
            });

        // Apply corrections sequentially (hot pixels are sparse, so this is fast)
        for (idx, value) in corrections {
            image.pixels[idx] = value;
        }
    }
}

/// Statistics for a single channel used in hot pixel detection.
#[derive(Debug)]
struct ChannelStats {
    median: f32,
    mad: f32,
    sigma: f32,
    threshold: f32,
}

/// Compute statistics and threshold for a single channel using MAD (Median Absolute Deviation).
fn compute_channel_stats(
    pixels: &[f32],
    channels: usize,
    channel: usize,
    sigma_threshold: f32,
) -> ChannelStats {
    let pixel_count = pixels.len() / channels;

    // Extract values for this channel in parallel
    let mut values: Vec<f32> = vec![0.0; pixel_count];
    const CHUNK_SIZE: usize = 4096;
    values
        .par_chunks_mut(CHUNK_SIZE)
        .enumerate()
        .for_each(|(chunk_idx, chunk)| {
            let start_pixel = chunk_idx * CHUNK_SIZE;
            for (i, v) in chunk.iter_mut().enumerate() {
                let pixel_idx = start_pixel + i;
                *v = pixels[pixel_idx * channels + channel];
            }
        });

    let median = crate::math::median_f32(&values);

    // Compute absolute deviations in parallel, reusing the same buffer
    values.par_chunks_mut(CHUNK_SIZE).for_each(|chunk| {
        for v in chunk.iter_mut() {
            *v = (*v - median).abs();
        }
    });
    let mad = crate::math::median_f32(&values);

    // Convert MAD to σ equivalent (for normal distribution, σ ≈ 1.4826 * MAD)
    const MAD_TO_SIGMA: f32 = 1.4826;
    let computed_sigma = mad * MAD_TO_SIGMA;

    // Apply minimum sigma floor: stacked master darks have compressed noise,
    // so use at least 1% of median to avoid overly tight thresholds
    let sigma = computed_sigma.max(median * 0.01);
    let threshold = median + sigma_threshold * sigma;

    ChannelStats {
        median,
        mad,
        sigma,
        threshold,
    }
}

/// Calculate median of 8-connected neighbors for a specific channel.
fn median_of_neighbors(image: &AstroImage, x: usize, y: usize, channel: usize) -> f32 {
    let width = image.dimensions.width;
    let height = image.dimensions.height;
    let channels = image.dimensions.channels;

    let mut neighbors = Vec::with_capacity(8);

    // Offsets for 8-connected neighbors
    let offsets: [(i32, i32); 8] = [
        (-1, -1),
        (0, -1),
        (1, -1),
        (-1, 0),
        (1, 0),
        (-1, 1),
        (0, 1),
        (1, 1),
    ];

    for (dx, dy) in offsets {
        let nx = x as i32 + dx;
        let ny = y as i32 + dy;

        if nx >= 0 && nx < width as i32 && ny >= 0 && ny < height as i32 {
            let idx = (ny as usize * width + nx as usize) * channels + channel;
            neighbors.push(image.pixels[idx]);
        }
    }

    if neighbors.is_empty() {
        // Edge case: isolated pixel with no neighbors (shouldn't happen in practice)
        let idx = (y * width + x) * channels + channel;
        return image.pixels[idx];
    }

    crate::math::median_f32(&neighbors)
}

#[cfg(feature = "bench")]
pub mod bench {
    //! Benchmark module for hot pixel detection.
    //! Run with: cargo bench --package lumos --features bench hot_pixels

    use super::HotPixelMap;
    use crate::{CalibrationMasters, StackingMethod};
    use criterion::{BenchmarkId, Criterion};
    use std::hint::black_box;
    use std::path::Path;

    /// Register hot pixel detection benchmarks with Criterion.
    pub fn benchmarks(c: &mut Criterion, masters_dir: &Path) {
        let masters = CalibrationMasters::load_from_directory(masters_dir, StackingMethod::Median)
            .expect("Failed to load calibration masters");

        let master_dark = masters
            .master_dark
            .expect("Master dark not found in calibration_masters directory");

        let mut group = c.benchmark_group("hot_pixels");
        group.sample_size(20);

        group.bench_function(BenchmarkId::new("from_master_dark", "sigma_5.0"), |b| {
            b.iter(|| black_box(HotPixelMap::from_master_dark(&master_dark, 5.0)))
        });

        group.finish();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::astro_image::AstroImageMetadata;

    fn make_test_image(
        width: usize,
        height: usize,
        channels: usize,
        pixels: Vec<f32>,
    ) -> AstroImage {
        AstroImage {
            metadata: AstroImageMetadata::default(),
            pixels,
            dimensions: ImageDimensions::new(width, height, channels),
        }
    }

    #[test]
    fn test_hot_pixel_detection_grayscale() {
        // 3x3 grayscale image with one hot pixel in center
        let pixels = vec![
            10.0, 10.0, 10.0, 10.0, 1000.0, 10.0, // center is hot
            10.0, 10.0, 10.0,
        ];
        let dark = make_test_image(3, 3, 1, pixels);

        let hot_map = HotPixelMap::from_master_dark(&dark, 5.0);

        assert_eq!(hot_map.count, 1);
        assert!(hot_map.is_hot_at(1, 1, 0)); // center is hot
        assert!(!hot_map.is_hot_at(0, 0, 0)); // corner is not hot
    }

    #[test]
    fn test_hot_pixel_detection_rgb() {
        // 2x2 RGB image with one hot pixel (red channel of pixel 1,1)
        let pixels = vec![
            10.0, 10.0, 10.0, // (0,0) RGB
            10.0, 10.0, 10.0, // (1,0) RGB
            10.0, 10.0, 10.0, // (0,1) RGB
            1000.0, 10.0, 10.0, // (1,1) RGB - red is hot
        ];
        let dark = make_test_image(2, 2, 3, pixels);

        let hot_map = HotPixelMap::from_master_dark(&dark, 5.0);

        assert_eq!(hot_map.count, 1);
        assert!(hot_map.is_hot_at(1, 1, 0)); // red channel at (1,1) is hot
        assert!(!hot_map.is_hot_at(1, 1, 1)); // green channel at (1,1) is not hot
    }

    #[test]
    fn test_hot_pixel_correction() {
        // 3x3 grayscale image with hot pixel in center
        let pixels = vec![
            10.0, 20.0, 30.0, 40.0, 1000.0, 50.0, // center is hot
            60.0, 70.0, 80.0,
        ];
        let mut image = make_test_image(3, 3, 1, pixels.clone());

        // Create hot pixel map manually (center pixel is hot)
        let hot_map = HotPixelMap {
            mask: vec![false, false, false, false, true, false, false, false, false],
            dimensions: image.dimensions,
            count: 1,
        };

        hot_map.correct(&mut image);

        // Center should be replaced with median of 8 neighbors
        // Neighbors: 10, 20, 30, 40, 50, 60, 70, 80 -> median = 45
        let center = image.pixels[4];
        assert!(
            (center - 45.0).abs() < f32::EPSILON,
            "Expected 45.0, got {}",
            center
        );

        // Other pixels should be unchanged
        assert_eq!(image.pixels[0], 10.0);
        assert_eq!(image.pixels[8], 80.0);
    }

    #[test]
    fn test_hot_pixel_correction_corner() {
        // 3x3 grayscale image with hot pixel in corner
        let pixels = vec![
            1000.0, 20.0, 30.0, // top-left is hot
            40.0, 50.0, 60.0, 70.0, 80.0, 90.0,
        ];
        let mut image = make_test_image(3, 3, 1, pixels);

        let hot_map = HotPixelMap {
            mask: vec![true, false, false, false, false, false, false, false, false],
            dimensions: image.dimensions,
            count: 1,
        };

        hot_map.correct(&mut image);

        // Corner has only 3 neighbors: 20, 40, 50 -> median = 40
        let corner = image.pixels[0];
        assert!(
            (corner - 40.0).abs() < f32::EPSILON,
            "Expected 40.0, got {}",
            corner
        );
    }

    #[test]
    fn test_hot_pixel_percentage() {
        let pixels = vec![10.0; 100];
        let mut dark = make_test_image(10, 10, 1, pixels);
        // Make 5 pixels hot
        dark.pixels[0] = 10000.0;
        dark.pixels[25] = 10000.0;
        dark.pixels[50] = 10000.0;
        dark.pixels[75] = 10000.0;
        dark.pixels[99] = 10000.0;

        let hot_map = HotPixelMap::from_master_dark(&dark, 5.0);

        assert_eq!(hot_map.count, 5);
        assert!((hot_map.percentage() - 5.0).abs() < f32::EPSILON);
    }

    #[test]
    #[should_panic(expected = "don't match")]
    fn test_correct_dimension_mismatch() {
        let image_pixels = vec![10.0; 9];
        let mut image = make_test_image(3, 3, 1, image_pixels);

        let hot_map = HotPixelMap {
            mask: vec![false; 4],
            dimensions: ImageDimensions::new(2, 2, 1),
            count: 0,
        };

        hot_map.correct(&mut image);
    }
}
