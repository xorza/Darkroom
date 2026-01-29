//! Benchmarks for deblending algorithms.
//! Run with: cargo bench -p lumos --features bench --bench star_detection_deblend

use super::DeblendConfig;
use super::local_maxima::{ComponentData, Pixel, deblend_local_maxima};
use super::multi_threshold::{MultiThresholdDeblendConfig, deblend_component};
use crate::common::Buffer2;
use criterion::{BenchmarkId, Criterion, Throughput};
use std::collections::HashMap;
use std::hint::black_box;

/// Benchmark data for deblending.
pub struct BenchData {
    pub image: Buffer2<f32>,
    pub components: Vec<ComponentData>,
}

impl BenchData {
    /// Create benchmark data with synthetic blended star pairs.
    pub fn new(width: usize, height: usize, num_pairs: usize, separation: usize) -> Self {
        let mut image_data = vec![0.0f32; width * height];
        let mut components = Vec::with_capacity(num_pairs);

        let sigma = 2.5f32;

        for i in 0..num_pairs {
            // Place star pairs in a grid
            let row = i / (width / (separation * 3));
            let col = i % (width / (separation * 3));

            let cx1 = col * separation * 3 + separation;
            let cy = row * separation * 3 + separation;
            let cx2 = cx1 + separation;

            if cx2 >= width - 10 || cy >= height - 10 {
                continue;
            }

            let amplitude1 = 1.0;
            let amplitude2 = 0.8;

            let mut pair_pixels: Vec<Pixel> = Vec::new();

            // Add first star
            let radius = (sigma * 4.0).ceil() as i32;
            for dy in -radius..=radius {
                for dx in -radius..=radius {
                    let x = (cx1 as i32 + dx) as usize;
                    let y = (cy as i32 + dy) as usize;
                    if x < width && y < height {
                        let r2 = (dx * dx + dy * dy) as f32;
                        let value = amplitude1 * (-r2 / (2.0 * sigma * sigma)).exp();
                        if value > 0.001 {
                            image_data[y * width + x] += value;
                            pair_pixels.push(Pixel {
                                x,
                                y,
                                value: image_data[y * width + x],
                            });
                        }
                    }
                }
            }

            // Add second star
            for dy in -radius..=radius {
                for dx in -radius..=radius {
                    let x = (cx2 as i32 + dx) as usize;
                    let y = (cy as i32 + dy) as usize;
                    if x < width && y < height {
                        let r2 = (dx * dx + dy * dy) as f32;
                        let value = amplitude2 * (-r2 / (2.0 * sigma * sigma)).exp();
                        if value > 0.001 {
                            image_data[y * width + x] += value;
                            pair_pixels.push(Pixel {
                                x,
                                y,
                                value: image_data[y * width + x],
                            });
                        }
                    }
                }
            }

            // Deduplicate
            let mut seen: HashMap<(usize, usize), f32> = HashMap::new();
            for p in pair_pixels {
                seen.insert((p.x, p.y), p.value);
            }
            let unique_pixels: Vec<Pixel> = seen
                .into_iter()
                .map(|((x, y), value)| Pixel { x, y, value })
                .collect();

            let data = ComponentData {
                x_min: unique_pixels.iter().map(|p| p.x).min().unwrap_or(0),
                x_max: unique_pixels.iter().map(|p| p.x).max().unwrap_or(0),
                y_min: unique_pixels.iter().map(|p| p.y).min().unwrap_or(0),
                y_max: unique_pixels.iter().map(|p| p.y).max().unwrap_or(0),
                pixels: unique_pixels,
            };

            components.push(data);
        }

        Self {
            image: Buffer2::new(width, height, image_data),
            components,
        }
    }
}

/// Benchmark local maxima deblending.
pub fn bench_local_maxima(data: &BenchData) -> usize {
    let config = DeblendConfig::default();
    let mut total_objects = 0;

    for component in &data.components {
        let result = deblend_local_maxima(component, &data.image, &config);
        total_objects += result.len();
    }

    total_objects
}

/// Benchmark multi-threshold deblending.
pub fn bench_multi_threshold(data: &BenchData) -> usize {
    let config = MultiThresholdDeblendConfig::default();
    let mut total_objects = 0;

    for component in &data.components {
        let result = deblend_component(&component.pixels, data.image.width(), 0.01, &config);
        total_objects += result.len();
    }

    total_objects
}

/// Register deblending benchmarks with Criterion.
pub fn benchmarks(c: &mut Criterion) {
    let mut local_group = c.benchmark_group("deblend_local_maxima");
    local_group.sample_size(50);

    for &(num_pairs, separation) in &[(10, 15), (50, 12), (100, 10)] {
        let data = BenchData::new(512, 512, num_pairs, separation);
        let config = DeblendConfig::default();
        let name = format!("{}pairs_sep{}", num_pairs, separation);

        local_group.throughput(Throughput::Elements(num_pairs as u64));
        local_group.bench_function(BenchmarkId::new("local_maxima", &name), |b| {
            b.iter(|| {
                for component in &data.components {
                    black_box(deblend_local_maxima(
                        black_box(component),
                        black_box(&data.image),
                        black_box(&config),
                    ));
                }
            })
        });
    }

    local_group.finish();

    let mut mt_group = c.benchmark_group("deblend_multi_threshold");
    mt_group.sample_size(30);

    for &(num_pairs, separation) in &[(10, 15), (50, 12)] {
        let data = BenchData::new(512, 512, num_pairs, separation);
        let config = MultiThresholdDeblendConfig::default();
        let name = format!("{}pairs_sep{}", num_pairs, separation);

        mt_group.throughput(Throughput::Elements(num_pairs as u64));
        mt_group.bench_function(BenchmarkId::new("multi_threshold", &name), |b| {
            b.iter(|| {
                for component in &data.components {
                    black_box(deblend_component(
                        black_box(&component.pixels),
                        data.image.width(),
                        0.01,
                        black_box(&config),
                    ));
                }
            })
        });
    }

    mt_group.finish();
}

#[cfg(test)]
mod bench_tests {
    use super::*;

    #[test]
    fn test_bench_data_creation() {
        let data = BenchData::new(256, 256, 10, 20);
        assert!(!data.components.is_empty());
    }

    #[test]
    fn test_bench_local_maxima() {
        let data = BenchData::new(256, 256, 10, 20);
        let count = bench_local_maxima(&data);
        assert!(count >= data.components.len());
    }

    #[test]
    fn test_bench_multi_threshold() {
        let data = BenchData::new(256, 256, 10, 20);
        let count = bench_multi_threshold(&data);
        assert!(count >= data.components.len());
    }
}
