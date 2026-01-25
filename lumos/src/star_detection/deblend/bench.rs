//! Benchmarks for deblending algorithms.

use super::DeblendConfig;
use super::local_maxima::{ComponentData, deblend_local_maxima};
use super::multi_threshold::{MultiThresholdDeblendConfig, deblend_component};
use std::collections::HashMap;

/// Benchmark data for deblending.
pub struct BenchData {
    pub image: Vec<f32>,
    pub components: Vec<ComponentData>,
    pub component_pixels: Vec<Vec<(usize, usize, f32)>>,
    pub width: usize,
    pub height: usize,
}

impl BenchData {
    /// Create benchmark data with synthetic blended star pairs.
    pub fn new(width: usize, height: usize, num_pairs: usize, separation: usize) -> Self {
        let mut image = vec![0.0f32; width * height];
        let mut components = Vec::with_capacity(num_pairs);
        let mut component_pixels = Vec::with_capacity(num_pairs);

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

            let mut pair_pixels: Vec<(usize, usize, f32)> = Vec::new();

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
                            image[y * width + x] += value;
                            pair_pixels.push((x, y, image[y * width + x]));
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
                            image[y * width + x] += value;
                            pair_pixels.push((x, y, image[y * width + x]));
                        }
                    }
                }
            }

            // Deduplicate
            let mut seen: HashMap<(usize, usize), f32> = HashMap::new();
            for (x, y, v) in pair_pixels {
                seen.insert((x, y), v);
            }
            let unique_pixels: Vec<_> = seen.into_iter().map(|((x, y), v)| (x, y, v)).collect();

            let data = ComponentData {
                x_min: unique_pixels.iter().map(|p| p.0).min().unwrap_or(0),
                x_max: unique_pixels.iter().map(|p| p.0).max().unwrap_or(0),
                y_min: unique_pixels.iter().map(|p| p.1).min().unwrap_or(0),
                y_max: unique_pixels.iter().map(|p| p.1).max().unwrap_or(0),
                pixels: unique_pixels.clone(),
            };

            components.push(data);
            component_pixels.push(unique_pixels);
        }

        Self {
            image,
            components,
            component_pixels,
            width,
            height,
        }
    }
}

/// Benchmark local maxima deblending.
pub fn bench_local_maxima(data: &BenchData) -> usize {
    let config = DeblendConfig::default();
    let mut total_objects = 0;

    for component in &data.components {
        let result = deblend_local_maxima(component, &data.image, data.width, &config);
        total_objects += result.len();
    }

    total_objects
}

/// Benchmark multi-threshold deblending.
pub fn bench_multi_threshold(data: &BenchData) -> usize {
    let config = MultiThresholdDeblendConfig::default();
    let mut total_objects = 0;

    for pixels in &data.component_pixels {
        let result = deblend_component(&data.image, pixels, data.width, 0.01, &config);
        total_objects += result.len();
    }

    total_objects
}

#[cfg(test)]
mod bench_tests {
    use super::*;

    #[test]
    fn test_bench_data_creation() {
        let data = BenchData::new(256, 256, 10, 20);
        assert!(!data.components.is_empty());
        assert_eq!(data.components.len(), data.component_pixels.len());
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
        assert!(count >= data.component_pixels.len());
    }
}
