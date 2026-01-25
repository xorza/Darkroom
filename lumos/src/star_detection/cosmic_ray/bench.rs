//! Benchmarks for cosmic ray detection.
//! Run with: cargo bench -p lumos --features bench --bench star_detection_cosmic_ray

use super::laplacian::compute_laplacian;
use super::{LACosmicConfig, detect_cosmic_rays};
use criterion::{BenchmarkId, Criterion, Throughput};
use std::hint::black_box;

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

/// Register cosmic ray detection benchmarks with Criterion.
pub fn benchmarks(c: &mut Criterion) {
    let mut laplacian_group = c.benchmark_group("cosmic_ray_laplacian");
    laplacian_group.sample_size(30);

    for &(width, height) in &[(512, 512), (1024, 1024), (2048, 2048)] {
        let data = BenchData::new(width, height, 100, 20);
        let size_name = format!("{}x{}", width, height);

        laplacian_group.throughput(Throughput::Elements((width * height) as u64));
        laplacian_group.bench_function(BenchmarkId::new("compute_laplacian", &size_name), |b| {
            b.iter(|| black_box(compute_laplacian(black_box(&data.pixels), width, height)))
        });
    }

    laplacian_group.finish();

    let mut detect_group = c.benchmark_group("cosmic_ray_detect");
    detect_group.sample_size(20);

    for &(width, height, num_stars, num_cr) in &[
        (512, 512, 50, 10),
        (1024, 1024, 200, 40),
        (2048, 2048, 800, 100),
    ] {
        let data = BenchData::new(width, height, num_stars, num_cr);
        let config = LACosmicConfig::default();
        let size_name = format!("{}x{}", width, height);

        detect_group.throughput(Throughput::Elements((width * height) as u64));
        detect_group.bench_function(BenchmarkId::new("detect_cosmic_rays", &size_name), |b| {
            b.iter(|| {
                black_box(detect_cosmic_rays(
                    black_box(&data.pixels),
                    width,
                    height,
                    black_box(&data.background),
                    black_box(&data.noise),
                    black_box(&config),
                ))
            })
        });
    }

    detect_group.finish();
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
