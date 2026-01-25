//! Benchmark module for centroid computation and profile fitting.
//! Run with: cargo bench --package lumos --features bench centroid

use super::gaussian_fit::{GaussianFitConfig, fit_gaussian_2d};
use super::moffat_fit::{MoffatFitConfig, fit_moffat_2d};
use super::{compute_centroid, compute_metrics, compute_stamp_radius, refine_centroid};
use crate::star_detection::StarDetectionConfig;
use crate::star_detection::background::BackgroundMap;
use crate::star_detection::detection::StarCandidate;
use criterion::{BenchmarkId, Criterion, Throughput};
use std::hint::black_box;

/// Generate a synthetic Gaussian star stamp for benchmarking.
fn generate_gaussian_stamp(
    width: usize,
    height: usize,
    cx: f32,
    cy: f32,
    amplitude: f32,
    sigma: f32,
    background: f32,
) -> Vec<f32> {
    let mut pixels = vec![background; width * height];
    for y in 0..height {
        for x in 0..width {
            let dx = x as f32 - cx;
            let dy = y as f32 - cy;
            let r2 = dx * dx + dy * dy;
            let value = amplitude * (-r2 / (2.0 * sigma * sigma)).exp();
            pixels[y * width + x] += value;
        }
    }
    pixels
}

/// Generate a synthetic Moffat star stamp for benchmarking.
fn generate_moffat_stamp(
    width: usize,
    height: usize,
    cx: f32,
    cy: f32,
    amplitude: f32,
    alpha: f32,
    beta: f32,
    background: f32,
) -> Vec<f32> {
    let mut pixels = vec![background; width * height];
    for y in 0..height {
        for x in 0..width {
            let dx = x as f32 - cx;
            let dy = y as f32 - cy;
            let r2 = dx * dx + dy * dy;
            let value = amplitude * (1.0 + r2 / (alpha * alpha)).powf(-beta);
            pixels[y * width + x] += value;
        }
    }
    pixels
}

/// Generate a field of synthetic stars for benchmarking centroid computation.
fn generate_star_field(
    width: usize,
    height: usize,
    num_stars: usize,
) -> (Vec<f32>, Vec<(f32, f32)>) {
    let background = 0.1f32;
    let noise = 0.01f32;
    let mut pixels = vec![background; width * height];

    // Add deterministic noise
    for (i, p) in pixels.iter_mut().enumerate() {
        let hash = ((i as u32).wrapping_mul(2654435761)) as f32 / u32::MAX as f32;
        *p += (hash - 0.5) * noise;
    }

    // Generate star positions deterministically
    let mut star_positions = Vec::with_capacity(num_stars);
    for star_idx in 0..num_stars {
        let hash1 = ((star_idx as u32).wrapping_mul(2654435761)) as usize;
        let hash2 = ((star_idx as u32).wrapping_mul(1597334677)) as usize;
        let hash3 = ((star_idx as u32).wrapping_mul(805306457)) as usize;

        let cx = 20.0 + (hash1 % (width - 40)) as f32 + 0.3; // Sub-pixel offset
        let cy = 20.0 + (hash2 % (height - 40)) as f32 + 0.7; // Sub-pixel offset
        let brightness = 0.5 + (hash3 % 500) as f32 / 1000.0;
        let sigma = 2.0;

        // Add Gaussian star
        for dy in -10i32..=10 {
            for dx in -10i32..=10 {
                let x = (cx as i32 + dx) as usize;
                let y = (cy as i32 + dy) as usize;
                if x < width && y < height {
                    let fx = x as f32 - cx;
                    let fy = y as f32 - cy;
                    let r2 = fx * fx + fy * fy;
                    let value = brightness * (-r2 / (2.0 * sigma * sigma)).exp();
                    pixels[y * width + x] += value;
                }
            }
        }
        star_positions.push((cx, cy));
    }

    (pixels, star_positions)
}

/// Create a simple background map for benchmarking.
fn create_background_map(width: usize, height: usize) -> BackgroundMap {
    let size = width * height;
    BackgroundMap {
        background: vec![0.1f32; size],
        noise: vec![0.01f32; size],
        width,
        height,
    }
}

/// Register centroid computation benchmarks with Criterion.
pub fn benchmarks(c: &mut Criterion) {
    let mut group = c.benchmark_group("centroid");
    group.sample_size(50);

    // Benchmark weighted centroid refinement
    {
        let width = 21;
        let height = 21;
        let cx = 10.3;
        let cy = 10.7;
        let pixels = generate_gaussian_stamp(width, height, cx, cy, 1.0, 2.0, 0.1);
        let background = create_background_map(width, height);
        let stamp_radius = 8;

        group.bench_function("refine_centroid_21x21", |b| {
            b.iter(|| {
                black_box(refine_centroid(
                    black_box(&pixels),
                    black_box(width),
                    black_box(height),
                    black_box(&background),
                    black_box(10.0),
                    black_box(10.0),
                    black_box(stamp_radius),
                ))
            })
        });
    }

    // Benchmark compute_metrics
    {
        let width = 21;
        let height = 21;
        let cx = 10.3;
        let cy = 10.7;
        let pixels = generate_gaussian_stamp(width, height, cx, cy, 1.0, 2.0, 0.1);
        let background = create_background_map(width, height);
        let stamp_radius = 8;

        group.bench_function("compute_metrics_21x21", |b| {
            b.iter(|| {
                black_box(compute_metrics(
                    black_box(&pixels),
                    black_box(width),
                    black_box(height),
                    black_box(&background),
                    black_box(cx),
                    black_box(cy),
                    black_box(stamp_radius),
                ))
            })
        });
    }

    // Benchmark full compute_centroid with varying star counts
    for num_stars in [10, 100, 500] {
        let (pixels, star_positions) = generate_star_field(512, 512, num_stars);
        let background = create_background_map(512, 512);
        let config = StarDetectionConfig::default();

        // Create star candidates from positions
        let candidates: Vec<StarCandidate> = star_positions
            .iter()
            .map(|&(x, y)| {
                let px = x.round() as usize;
                let py = y.round() as usize;
                let idx = py * 512 + px;
                StarCandidate {
                    x_min: px.saturating_sub(5),
                    x_max: (px + 5).min(511),
                    y_min: py.saturating_sub(5),
                    y_max: (py + 5).min(511),
                    peak_x: px,
                    peak_y: py,
                    peak_value: pixels[idx],
                    area: 100,
                }
            })
            .collect();

        group.throughput(Throughput::Elements(num_stars as u64));
        group.bench_function(BenchmarkId::new("compute_centroid_batch", num_stars), |b| {
            b.iter(|| {
                for candidate in &candidates {
                    black_box(compute_centroid(
                        black_box(&pixels),
                        black_box(512),
                        black_box(512),
                        black_box(&background),
                        black_box(candidate),
                        black_box(&config),
                    ));
                }
            })
        });
    }

    group.finish();

    // Gaussian fitting benchmarks
    let mut gaussian_group = c.benchmark_group("gaussian_fit");
    gaussian_group.sample_size(50);

    for stamp_size in [15, 21, 31] {
        let width = stamp_size;
        let height = stamp_size;
        let cx = (stamp_size / 2) as f32 + 0.3;
        let cy = (stamp_size / 2) as f32 + 0.7;
        let pixels = generate_gaussian_stamp(width, height, cx, cy, 1.0, 2.5, 0.1);
        let config = GaussianFitConfig::default();

        gaussian_group.bench_function(BenchmarkId::new("fit_gaussian_2d", stamp_size), |b| {
            b.iter(|| {
                black_box(fit_gaussian_2d(
                    black_box(&pixels),
                    black_box(width),
                    black_box(height),
                    black_box((stamp_size / 2) as f32),
                    black_box((stamp_size / 2) as f32),
                    black_box(stamp_size / 2 - 2),
                    black_box(0.1),
                    black_box(&config),
                ))
            })
        });
    }

    gaussian_group.finish();

    // Moffat fitting benchmarks
    let mut moffat_group = c.benchmark_group("moffat_fit");
    moffat_group.sample_size(50);

    for stamp_size in [15, 21, 31] {
        let width = stamp_size;
        let height = stamp_size;
        let cx = (stamp_size / 2) as f32 + 0.3;
        let cy = (stamp_size / 2) as f32 + 0.7;
        let pixels = generate_moffat_stamp(width, height, cx, cy, 1.0, 2.5, 2.5, 0.1);

        // Fixed beta (faster)
        let config_fixed = MoffatFitConfig {
            fit_beta: false,
            fixed_beta: 2.5,
            ..Default::default()
        };

        moffat_group.bench_function(
            BenchmarkId::new("fit_moffat_2d_fixed_beta", stamp_size),
            |b| {
                b.iter(|| {
                    black_box(fit_moffat_2d(
                        black_box(&pixels),
                        black_box(width),
                        black_box(height),
                        black_box((stamp_size / 2) as f32),
                        black_box((stamp_size / 2) as f32),
                        black_box(stamp_size / 2 - 2),
                        black_box(0.1),
                        black_box(&config_fixed),
                    ))
                })
            },
        );

        // Variable beta (slower)
        let config_var = MoffatFitConfig {
            fit_beta: true,
            fixed_beta: 2.5,
            ..Default::default()
        };

        moffat_group.bench_function(
            BenchmarkId::new("fit_moffat_2d_var_beta", stamp_size),
            |b| {
                b.iter(|| {
                    black_box(fit_moffat_2d(
                        black_box(&pixels),
                        black_box(width),
                        black_box(height),
                        black_box((stamp_size / 2) as f32),
                        black_box((stamp_size / 2) as f32),
                        black_box(stamp_size / 2 - 2),
                        black_box(0.1),
                        black_box(&config_var),
                    ))
                })
            },
        );
    }

    moffat_group.finish();

    // Stamp radius computation benchmark
    let mut util_group = c.benchmark_group("centroid_utils");
    util_group.bench_function("compute_stamp_radius", |b| {
        b.iter(|| {
            for fwhm in [2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0, 9.0, 10.0] {
                black_box(compute_stamp_radius(black_box(fwhm)));
            }
        })
    });
    util_group.finish();
}
