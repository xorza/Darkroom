//! Benchmark module for centroid computation and profile fitting.
//! Run with: cargo bench --package lumos --features bench centroid

use super::gaussian_fit::{GaussianFitConfig, fit_gaussian_2d};
use super::moffat_fit::{MoffatFitConfig, fit_moffat_2d};
use super::{compute_centroid, compute_metrics, refine_centroid};
use crate::star_detection::StarDetectionConfig;
use crate::star_detection::common::compute_stamp_radius;
use crate::star_detection::detection::StarCandidate;
use crate::testing::synthetic::{background_map, stamps};
use criterion::{BenchmarkId, Criterion, Throughput};
use std::hint::black_box;

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
        let pixels = stamps::gaussian(width, height, cx, cy, 2.0, 1.0, 0.1);
        let background = background_map::uniform(width, height, 0.1, 0.01);
        let stamp_radius = 8;

        group.bench_function("refine_centroid_21x21", |b| {
            b.iter(|| {
                black_box(refine_centroid(
                    black_box(pixels.as_ref()),
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
        let pixels = stamps::gaussian(width, height, cx, cy, 2.0, 1.0, 0.1);
        let background = background_map::uniform(width, height, 0.1, 0.01);
        let stamp_radius = 8;

        group.bench_function("compute_metrics_21x21", |b| {
            b.iter(|| {
                black_box(compute_metrics(
                    black_box(pixels.as_ref()),
                    black_box(width),
                    black_box(height),
                    black_box(&background),
                    black_box(cx),
                    black_box(cy),
                    black_box(stamp_radius),
                    black_box(None),
                    black_box(None),
                ))
            })
        });
    }

    // Benchmark full compute_centroid with varying star counts
    for num_stars in [10, 100, 500] {
        let (pixels, star_positions) = stamps::star_field(512, 512, num_stars, 2.0, 0.1, 42);
        let background = background_map::uniform(512, 512, 0.1, 0.01);
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
        let cx = (stamp_size / 2) as f32 + 0.3;
        let cy = (stamp_size / 2) as f32 + 0.7;
        let pixels = stamps::gaussian(stamp_size, stamp_size, cx, cy, 2.5, 1.0, 0.1);
        let config = GaussianFitConfig::default();

        gaussian_group.bench_function(BenchmarkId::new("fit_gaussian_2d", stamp_size), |b| {
            b.iter(|| {
                black_box(fit_gaussian_2d(
                    black_box(&pixels),
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
        let cx = (stamp_size / 2) as f32 + 0.3;
        let cy = (stamp_size / 2) as f32 + 0.7;
        let pixels = stamps::moffat(stamp_size, stamp_size, cx, cy, 2.5, 2.5, 1.0, 0.1);

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
