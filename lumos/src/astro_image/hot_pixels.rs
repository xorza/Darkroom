//! Hot pixel detection and correction.
//!
//! Detects defective sensor pixels from master dark frames and corrects them
//! by replacing with the median of 8-connected neighbors.
//!
//! # Algorithm
//!
//! Uses **Median Absolute Deviation (MAD)** for robust σ estimation:
//!
//! 1. **Why MAD instead of standard deviation?**
//!    Standard deviation is heavily influenced by outliers - the very pixels we're
//!    trying to detect. MAD is robust: even if 49% of pixels are outliers, the
//!    median (and thus MAD) remains accurate.
//!
//! 2. **The 1.4826 constant (MAD to σ conversion):**
//!    For a normal distribution, MAD ≈ 0.6745 × σ. Therefore σ ≈ 1.4826 × MAD.
//!    This constant comes from the inverse of the 75th percentile of the standard
//!    normal distribution: 1/Φ⁻¹(0.75) ≈ 1.4826.
//!
//! 3. **Per-channel analysis:**
//!    Each color channel (R, G, B) has different noise characteristics due to
//!    sensor design and Bayer pattern. Analyzing channels separately prevents
//!    false positives from channel-to-channel variation.
//!
//! 4. **Adaptive sampling for large images:**
//!    For images >200K pixels, exact median computation is slow. We sample 100K
//!    pixels uniformly, which gives <0.5% median error with >99% confidence
//!    (by the central limit theorem for order statistics).

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

        let pixels = master_dark.pixels();
        let channels = master_dark.channels();
        let pixel_count = master_dark.width() * master_dark.height();

        // Compute per-channel thresholds in a single pass
        let channel_stats = compute_all_channel_stats(pixels, channels, sigma_threshold);

        tracing::debug!(
            "Hot pixel detection per-channel stats ({}x{}x{}):",
            master_dark.width(),
            master_dark.height(),
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
            dimensions: master_dark.dimensions(),
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
            image.dimensions() == self.dimensions,
            "Image dimensions {:?} don't match hot pixel map {:?}",
            image.dimensions(),
            self.dimensions
        );

        // Early exit if no hot pixels
        if self.count == 0 {
            return;
        }

        let width = image.width();
        let height = image.height();
        let channels = image.channels();
        let row_stride = width * channels;

        // Pre-compute all corrections in parallel chunks for cache locality
        // Processing consecutive rows together keeps neighbor data in cache
        // Hot pixels are sparse (~0.01-0.1%), so total corrections are small
        const ROW_CHUNK_SIZE: usize = 64;

        let corrections: Vec<(usize, f32)> = (0..height.div_ceil(ROW_CHUNK_SIZE))
            .into_par_iter()
            .flat_map(|chunk_idx| {
                let start_y = chunk_idx * ROW_CHUNK_SIZE;
                let end_y = (start_y + ROW_CHUNK_SIZE).min(height);

                let mut chunk_corrections = Vec::new();
                for y in start_y..end_y {
                    for x in 0..width {
                        for c in 0..channels {
                            let idx = y * row_stride + x * channels + c;
                            if self.is_hot(idx) {
                                let replacement = median_of_neighbors(image, x, y, c);
                                chunk_corrections.push((idx, replacement));
                            }
                        }
                    }
                }
                chunk_corrections
            })
            .collect();

        // Apply corrections
        let pixels = image.pixels_mut();
        for (idx, value) in corrections {
            pixels[idx] = value;
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

/// Maximum number of samples to use for median estimation.
/// 100K samples gives <0.5% error for median estimation with very high confidence.
const MAX_MEDIAN_SAMPLES: usize = 100_000;

/// Compute statistics and thresholds for all channels using sampled median.
///
/// For large images, computing exact median is expensive (O(n) but with large constant).
/// Instead, we sample a subset of pixels and compute median on the sample.
/// With 100K samples, the median estimate is accurate to within ~0.5% with high probability.
///
/// Samples all channels in a single pass for better cache locality.
fn compute_all_channel_stats(
    pixels: &[f32],
    channels: usize,
    sigma_threshold: f32,
) -> Vec<ChannelStats> {
    let pixel_count = pixels.len() / channels;

    // Determine sampling strategy
    let use_sampling = pixel_count > MAX_MEDIAN_SAMPLES * 2;
    let sample_count = if use_sampling {
        MAX_MEDIAN_SAMPLES
    } else {
        pixel_count
    };

    // Calculate stride for uniform sampling across the image
    let stride = if use_sampling {
        pixel_count / sample_count
    } else {
        1
    };

    // Allocate sample buffers for all channels
    let mut channel_samples: Vec<Vec<f32>> = (0..channels)
        .map(|_| Vec::with_capacity(sample_count))
        .collect();

    // Extract samples for all channels in a single pass - contiguous memory access
    for i in 0..sample_count {
        let base = i * stride * channels;
        for c in 0..channels {
            channel_samples[c].push(pixels[base + c]);
        }
    }

    // Compute stats for each channel
    channel_samples
        .into_iter()
        .map(|mut samples| {
            let median = crate::math::median_f32_mut(&mut samples);

            // Compute absolute deviations in place, reusing the buffer
            for v in samples.iter_mut() {
                *v = (*v - median).abs();
            }
            let mad = crate::math::median_f32_mut(&mut samples);

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
        })
        .collect()
}

/// Calculate median of 8-connected neighbors for a specific channel.
fn median_of_neighbors(image: &AstroImage, x: usize, y: usize, channel: usize) -> f32 {
    let width = image.width();
    let height = image.height();
    let channels = image.channels();
    let pixels = image.pixels();

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
            neighbors.push(pixels[idx]);
        }
    }

    if neighbors.is_empty() {
        // Edge case: isolated pixel with no neighbors (shouldn't happen in practice)
        let idx = (y * width + x) * channels + channel;
        return pixels[idx];
    }

    crate::math::median_f32_mut(&mut neighbors)
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
        let masters =
            CalibrationMasters::load_from_directory(masters_dir, StackingMethod::default())
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

    fn make_test_image(
        width: usize,
        height: usize,
        channels: usize,
        pixels: Vec<f32>,
    ) -> AstroImage {
        AstroImage::new(width, height, channels, pixels)
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
            dimensions: image.dimensions(),
            count: 1,
        };

        hot_map.correct(&mut image);

        // Center should be replaced with median of 8 neighbors
        // Neighbors: 10, 20, 30, 40, 50, 60, 70, 80 -> median = 45
        let center = image.pixels()[4];
        assert!(
            (center - 45.0).abs() < f32::EPSILON,
            "Expected 45.0, got {}",
            center
        );

        // Other pixels should be unchanged
        assert_eq!(image.pixels()[0], 10.0);
        assert_eq!(image.pixels()[8], 80.0);
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
            dimensions: image.dimensions(),
            count: 1,
        };

        hot_map.correct(&mut image);

        // Corner has only 3 neighbors: 20, 40, 50 -> median = 40
        let corner = image.pixels()[0];
        assert!(
            (corner - 40.0).abs() < f32::EPSILON,
            "Expected 40.0, got {}",
            corner
        );
    }

    #[test]
    fn test_hot_pixel_percentage() {
        let mut pixels = vec![10.0; 100];
        // Make 5 pixels hot
        pixels[0] = 10000.0;
        pixels[25] = 10000.0;
        pixels[50] = 10000.0;
        pixels[75] = 10000.0;
        pixels[99] = 10000.0;
        let dark = make_test_image(10, 10, 1, pixels);

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

    #[test]
    fn test_sampled_median_accuracy() {
        // Test that sampled median gives accurate results on large data
        // Create a 1000x1000 grayscale image (1M pixels, triggers sampling)
        let size = 1000;
        let pixel_count = size * size;

        // Create data with known distribution: values 0..pixel_count
        // True median should be (pixel_count - 1) / 2 = 499999.5
        let pixels: Vec<f32> = (0..pixel_count).map(|i| i as f32).collect();

        // Get stats using our sampled approach
        let stats = super::compute_all_channel_stats(&pixels, 1, 5.0);

        // Exact median
        let exact_median = (pixel_count - 1) as f32 / 2.0;

        // Sampled median should be within 1% of exact
        let error_pct = (stats[0].median - exact_median).abs() / exact_median * 100.0;
        assert!(
            error_pct < 1.0,
            "Sampled median {} differs from exact {} by {:.2}%",
            stats[0].median,
            exact_median,
            error_pct
        );
    }

    #[test]
    fn test_hot_pixel_detection_large_image() {
        // Test hot pixel detection on an image large enough to trigger sampling
        let size = 500;
        let pixel_count = size * size;

        // Create image with uniform background and a few hot pixels
        let mut pixels: Vec<f32> = vec![100.0; pixel_count];

        // Add 10 hot pixels (0.004% of total)
        let hot_indices = [
            0, 1000, 5000, 10000, 50000, 100000, 150000, 200000, 240000, 249999,
        ];
        for &idx in &hot_indices {
            pixels[idx] = 10000.0; // 100x background
        }

        let dark = make_test_image(size, size, 1, pixels);
        let hot_map = HotPixelMap::from_master_dark(&dark, 5.0);

        // Should detect all 10 hot pixels
        assert_eq!(hot_map.count, 10);

        // Verify specific hot pixels are detected
        for &idx in &hot_indices {
            assert!(hot_map.is_hot(idx), "Hot pixel at {} not detected", idx);
        }
    }
}
