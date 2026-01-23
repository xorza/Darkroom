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
    /// Hot pixels are defined as pixels with values > sigma_threshold * σ above the median.
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
        let median = crate::math::median_f32(pixels);

        // Use MAD (Median Absolute Deviation) for robust std dev estimation
        // MAD is not affected by outliers like regular std dev
        let abs_deviations: Vec<f32> = pixels.iter().map(|&p| (p - median).abs()).collect();
        let mad = crate::math::median_f32(&abs_deviations);

        // Convert MAD to σ equivalent (for normal distribution, σ ≈ 1.4826 * MAD)
        const MAD_TO_SIGMA: f32 = 1.4826;
        let robust_sigma = mad * MAD_TO_SIGMA;

        let threshold = median + sigma_threshold * robust_sigma;

        // Detect hot pixels in parallel
        let mask: Vec<bool> = pixels.par_iter().map(|&p| p > threshold).collect();

        let count = mask.iter().filter(|&&b| b).count();

        Self {
            mask,
            dimensions: master_dark.dimensions,
            count,
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

    /// Get the percentage of hot pixels.
    pub fn percentage(&self) -> f32 {
        100.0 * self.count as f32 / self.mask.len() as f32
    }
}

/// Correct hot pixels in an image by replacing them with median of 8-connected neighbors.
///
/// # Arguments
/// * `image` - The image to correct (modified in place)
/// * `hot_pixel_map` - Map of hot pixels to correct
///
/// # Panics
/// Panics if hot_pixel_map dimensions don't match image dimensions.
pub fn correct_hot_pixels(image: &mut AstroImage, hot_pixel_map: &HotPixelMap) {
    assert!(
        image.dimensions == hot_pixel_map.dimensions,
        "Hot pixel map dimensions {:?} don't match image {:?}",
        hot_pixel_map.dimensions,
        image.dimensions
    );

    let width = image.dimensions.width;
    let height = image.dimensions.height;
    let channels = image.dimensions.channels;

    // Collect corrections first (can't mutate while iterating)
    let corrections: Vec<(usize, f32)> = (0..height)
        .into_par_iter()
        .flat_map(|y| {
            let mut local_corrections = Vec::new();
            for x in 0..width {
                for c in 0..channels {
                    let idx = (y * width + x) * channels + c;
                    if hot_pixel_map.is_hot(idx) {
                        let replacement = median_of_neighbors(image, x, y, c);
                        local_corrections.push((idx, replacement));
                    }
                }
            }
            local_corrections
        })
        .collect();

    // Apply corrections
    for (idx, value) in corrections {
        image.pixels[idx] = value;
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

        correct_hot_pixels(&mut image, &hot_map);

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

        correct_hot_pixels(&mut image, &hot_map);

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
    fn test_correct_hot_pixels_dimension_mismatch() {
        let image_pixels = vec![10.0; 9];
        let mut image = make_test_image(3, 3, 1, image_pixels);

        let hot_map = HotPixelMap {
            mask: vec![false; 4],
            dimensions: ImageDimensions::new(2, 2, 1),
            count: 0,
        };

        correct_hot_pixels(&mut image, &hot_map);
    }
}
