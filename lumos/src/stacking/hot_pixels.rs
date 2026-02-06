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

use crate::astro_image::{AstroImage, ImageDimensions};

/// Per-channel hot pixel indices - stores only the indices of hot pixels (sparse).
///
/// For a typical image with 0.01-0.1% hot pixels, storing indices uses far less
/// memory than a full boolean mask (e.g., 80KB vs 24MB for a 24MP image with 10K hot pixels).
#[derive(Debug, Clone)]
pub enum HotPixelMask {
    /// Grayscale image (1 channel) - indices of hot pixels
    L(Vec<usize>),
    /// RGB image (3 channels) - per-channel indices of hot pixels
    Rgb([Vec<usize>; 3]),
}

/// A mask of hot (defective) pixels detected from a master dark frame.
#[derive(Debug, Clone)]
pub struct HotPixelMap {
    /// Per-channel boolean masks where true = hot pixel
    mask: HotPixelMask,
    /// Image dimensions
    pub dimensions: ImageDimensions,
    /// Number of unique hot pixel positions (a pixel counts once even if multiple channels are hot)
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

        let channels = master_dark.channels();
        let pixel_count = master_dark.width() * master_dark.height();

        // Compute per-channel thresholds
        let channel_stats = compute_all_channel_stats(master_dark, sigma_threshold);

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

        // Helper to collect hot pixel indices for a single channel
        let collect_hot_indices = |c: usize| -> Vec<usize> {
            let channel_data = master_dark.channel(c);
            let threshold = thresholds[c];

            // Process in chunks for better cache locality and reduced task overhead
            const CHUNK_SIZE: usize = 64 * 1024;
            channel_data
                .par_chunks(CHUNK_SIZE)
                .enumerate()
                .flat_map(|(chunk_idx, chunk)| {
                    let base_idx = chunk_idx * CHUNK_SIZE;
                    chunk
                        .iter()
                        .enumerate()
                        .filter(|&(_, &val)| val > threshold)
                        .map(|(i, _)| base_idx + i)
                        .collect::<Vec<_>>()
                })
                .collect()
        };

        // Detect hot pixels per channel - stored as sparse indices
        let (mask, pixel_hot_count) = if channels == 1 {
            let indices = collect_hot_indices(0);
            let count = indices.len();
            (HotPixelMask::L(indices), count)
        } else {
            let (r, (g, b)) = rayon::join(
                || collect_hot_indices(0),
                || rayon::join(|| collect_hot_indices(1), || collect_hot_indices(2)),
            );

            // Count unique hot pixel positions using a temporary boolean mask
            // (more efficient than sorting and deduping for sparse data)
            let mut seen = vec![false; pixel_count];
            for &idx in r.iter().chain(g.iter()).chain(b.iter()) {
                seen[idx] = true;
            }
            let count = seen.iter().filter(|&&v| v).count();

            (HotPixelMask::Rgb([r, g, b]), count)
        };

        Self {
            mask,
            dimensions: master_dark.dimensions(),
            count: pixel_hot_count,
        }
    }

    /// Get the hot pixel indices for a specific channel.
    #[inline]
    pub fn channel_indices(&self, channel: usize) -> &[usize] {
        match &self.mask {
            HotPixelMask::L(indices) => {
                debug_assert!(channel == 0);
                indices
            }
            HotPixelMask::Rgb(channels) => &channels[channel],
        }
    }

    /// Get the percentage of hot pixels (as percentage of total pixels, not channel values).
    pub fn percentage(&self) -> f32 {
        let pixel_count = self.dimensions.width * self.dimensions.height;
        100.0 * self.count as f32 / pixel_count as f32
    }

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

        // Helper to correct a single channel.
        let correct_channel = |hot_indices: &[usize], channel_data: &mut [f32]| {
            for &pixel_idx in hot_indices {
                let x = pixel_idx % width;
                let y = pixel_idx / width;
                channel_data[pixel_idx] =
                    median_of_neighbors_raw(channel_data, width, height, x, y);
            }
        };

        match &self.mask {
            HotPixelMask::L(indices) => {
                correct_channel(indices, image.channel_mut(0));
            }
            HotPixelMask::Rgb(channel_indices) => {
                // Process channels in parallel using apply_per_channel_mut
                image.apply_per_channel_mut(|c, channel_data| {
                    correct_channel(&channel_indices[c], channel_data);
                });
            }
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
fn compute_all_channel_stats(image: &AstroImage, sigma_threshold: f32) -> Vec<ChannelStats> {
    let channels = image.channels();
    let pixel_count = image.width() * image.height();

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

    // Compute stats for each channel - planar data allows direct access
    (0..channels)
        .map(|c| {
            let channel_data = image.channel(c);

            // Sample the channel data
            let mut samples: Vec<f32> = (0..sample_count)
                .map(|i| channel_data[i * stride])
                .collect();

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

/// Calculate median of 8-connected neighbors from raw channel data.
fn median_of_neighbors_raw(
    channel_data: &[f32],
    width: usize,
    height: usize,
    x: usize,
    y: usize,
) -> f32 {
    let mut neighbors: [f32; 8] = [0.0; 8];
    let mut count = 0;

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
            let idx = ny as usize * width + nx as usize;
            neighbors[count] = channel_data[idx];
            count += 1;
        }
    }

    if count == 0 {
        // Edge case: isolated pixel with no neighbors (shouldn't happen in practice)
        let idx = y * width + x;
        return channel_data[idx];
    }

    crate::math::median_f32_mut(&mut neighbors[..count])
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
        AstroImage::from_pixels(ImageDimensions::new(width, height, channels), pixels)
    }

    /// Test helper: check if a pixel index is hot in a channel (uses binary search)
    fn is_hot(hot_map: &HotPixelMap, channel: usize, pixel_idx: usize) -> bool {
        hot_map
            .channel_indices(channel)
            .binary_search(&pixel_idx)
            .is_ok()
    }

    /// Test helper: check if a pixel at (x, y) is hot in a channel
    fn is_hot_at(hot_map: &HotPixelMap, x: usize, y: usize, channel: usize) -> bool {
        let idx = y * hot_map.dimensions.width + x;
        is_hot(hot_map, channel, idx)
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
        assert!(is_hot_at(&hot_map, 1, 1, 0)); // center is hot
        assert!(!is_hot_at(&hot_map, 0, 0, 0)); // corner is not hot
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
        assert!(is_hot_at(&hot_map, 1, 1, 0)); // red channel at (1,1) is hot
        assert!(!is_hot_at(&hot_map, 1, 1, 1)); // green channel at (1,1) is not hot
    }

    #[test]
    fn test_hot_pixel_correction() {
        // 3x3 grayscale image with hot pixel in center
        let pixels = vec![
            10.0, 20.0, 30.0, 40.0, 1000.0, 50.0, // center is hot
            60.0, 70.0, 80.0,
        ];
        let mut image = make_test_image(3, 3, 1, pixels.clone());

        // Create hot pixel map manually (center pixel at index 4 is hot)
        let hot_map = HotPixelMap {
            mask: HotPixelMask::L(vec![4]),
            dimensions: image.dimensions(),
            count: 1,
        };

        hot_map.correct(&mut image);

        // Center should be replaced with median of 8 neighbors
        // Neighbors: 10, 20, 30, 40, 50, 60, 70, 80 -> median = 45
        let center = image.channel(0)[4];
        assert!(
            (center - 45.0).abs() < f32::EPSILON,
            "Expected 45.0, got {}",
            center
        );

        // Other pixels should be unchanged
        assert_eq!(image.channel(0)[0], 10.0);
        assert_eq!(image.channel(0)[8], 80.0);
    }

    #[test]
    fn test_hot_pixel_correction_corner() {
        // 3x3 grayscale image with hot pixel in corner
        let pixels = vec![
            1000.0, 20.0, 30.0, // top-left is hot
            40.0, 50.0, 60.0, 70.0, 80.0, 90.0,
        ];
        let mut image = make_test_image(3, 3, 1, pixels);

        // Hot pixel at index 0 (top-left corner)
        let hot_map = HotPixelMap {
            mask: HotPixelMask::L(vec![0]),
            dimensions: image.dimensions(),
            count: 1,
        };

        hot_map.correct(&mut image);

        // Corner has only 3 neighbors: 20, 40, 50 -> median = 40
        let corner = image.channel(0)[0];
        assert!(
            (corner - 40.0).abs() < f32::EPSILON,
            "Expected 40.0, got {}",
            corner
        );
    }

    #[test]
    fn test_hot_pixel_correction_rgb() {
        // 3x3 RGB image with hot pixels in different channels
        // Pixel layout (interleaved RGB input):
        // (0,0) (1,0) (2,0)
        // (0,1) (1,1) (2,1)
        // (0,2) (1,2) (2,2)
        let pixels = vec![
            10.0, 10.0, 10.0, // (0,0)
            10.0, 10.0, 10.0, // (1,0)
            10.0, 10.0, 10.0, // (2,0)
            10.0, 10.0, 10.0, // (0,1)
            1000.0, 10.0, 1000.0, // (1,1) - R and B channels hot
            10.0, 10.0, 10.0, // (2,1)
            10.0, 10.0, 10.0, // (0,2)
            10.0, 10.0, 10.0, // (1,2)
            10.0, 10.0, 10.0, // (2,2)
        ];
        let mut image = make_test_image(3, 3, 3, pixels);

        // Center pixel (1,1) = index 4 is hot in R and B channels
        let hot_map = HotPixelMap {
            mask: HotPixelMask::Rgb([vec![4], vec![], vec![4]]),
            dimensions: image.dimensions(),
            count: 1,
        };

        hot_map.correct(&mut image);

        // R and B channels at center should be corrected to median of neighbors (10.0)
        assert_eq!(image.channel(0)[4], 10.0); // R corrected
        assert_eq!(image.channel(1)[4], 10.0); // G unchanged
        assert_eq!(image.channel(2)[4], 10.0); // B corrected
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
            mask: HotPixelMask::L(vec![]),
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
        let image = make_test_image(size, size, 1, pixels);

        // Get stats using our sampled approach
        let stats = super::compute_all_channel_stats(&image, 5.0);

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

        // Verify specific hot pixels are detected (channel 0 for grayscale)
        for &idx in &hot_indices {
            assert!(
                is_hot(&hot_map, 0, idx),
                "Hot pixel at {} not detected",
                idx
            );
        }
    }
}
