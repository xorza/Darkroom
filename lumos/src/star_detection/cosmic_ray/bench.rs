//! Benchmarks for cosmic ray detection.

use super::laplacian::compute_laplacian;
use super::{LACosmicConfig, detect_cosmic_rays};

/// Benchmark data for cosmic ray detection.
pub struct BenchData {
    pub pixels: Vec<f32>,
    pub background: Vec<f32>,
    pub noise: Vec<f32>,
    pub width: usize,
    pub height: usize,
}

impl BenchData {
    /// Create benchmark data with synthetic stars and cosmic rays.
    pub fn new(width: usize, height: usize, num_stars: usize, num_cosmic_rays: usize) -> Self {
        let mut pixels = vec![0.1f32; width * height];
        let background = vec![0.1f32; width * height];
        let noise = vec![0.01f32; width * height];

        // Add Gaussian stars
        let sigma = 2.5f32;
        for i in 0..num_stars {
            let cx = ((i * 97) % width) as f32;
            let cy = ((i * 73) % height) as f32;
            let amplitude = 0.5 + (i % 5) as f32 * 0.1;

            let radius = (sigma * 4.0).ceil() as i32;
            for dy in -radius..=radius {
                for dx in -radius..=radius {
                    let x = (cx as i32 + dx) as usize;
                    let y = (cy as i32 + dy) as usize;
                    if x < width && y < height {
                        let r2 = (dx * dx + dy * dy) as f32;
                        let value = amplitude * (-r2 / (2.0 * sigma * sigma)).exp();
                        pixels[y * width + x] += value;
                    }
                }
            }
        }

        // Add cosmic rays (single pixels)
        for i in 0..num_cosmic_rays {
            let x = ((i * 127 + 31) % width).max(1).min(width - 2);
            let y = ((i * 89 + 17) % height).max(1).min(height - 2);
            pixels[y * width + x] = 0.9;
        }

        Self {
            pixels,
            background,
            noise,
            width,
            height,
        }
    }
}

/// Run Laplacian computation benchmark.
pub fn bench_laplacian(data: &BenchData) -> Vec<f32> {
    compute_laplacian(&data.pixels, data.width, data.height)
}

/// Run full cosmic ray detection benchmark.
pub fn bench_detect_cosmic_rays(data: &BenchData) -> usize {
    let config = LACosmicConfig::default();
    let result = detect_cosmic_rays(
        &data.pixels,
        data.width,
        data.height,
        &data.background,
        &data.noise,
        &config,
    );
    result.cosmic_ray_count
}

#[cfg(test)]
mod bench_tests {
    use super::*;

    #[test]
    fn test_bench_data_creation() {
        let data = BenchData::new(256, 256, 50, 10);
        assert_eq!(data.pixels.len(), 256 * 256);
        assert_eq!(data.background.len(), 256 * 256);
        assert_eq!(data.noise.len(), 256 * 256);
    }

    #[test]
    fn test_bench_laplacian() {
        let data = BenchData::new(128, 128, 20, 5);
        let laplacian = bench_laplacian(&data);
        assert_eq!(laplacian.len(), 128 * 128);
    }

    #[test]
    fn test_bench_detect_cosmic_rays() {
        let data = BenchData::new(128, 128, 20, 5);
        let count = bench_detect_cosmic_rays(&data);
        // Should detect at least some cosmic rays
        assert!(count > 0, "Should detect some cosmic rays");
    }
}
