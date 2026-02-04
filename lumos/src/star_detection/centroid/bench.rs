//! Benchmarks for centroid computation.
//!
//! Run with: `cargo test -p lumos --release bench_centroid -- --ignored --nocapture`

use ::bench::quick_bench;
use glam::Vec2;
use std::hint::black_box;

use super::gaussian_fit::{GaussianFitConfig, fit_gaussian_2d};
use super::measure_star;
use super::moffat_fit::{MoffatFitConfig, fit_moffat_2d};
use super::refine_centroid;
use crate::common::Buffer2;
use crate::star_detection::BackgroundEstimate;
use crate::star_detection::config::{CentroidMethod, Config, LocalBackgroundMethod};
use crate::star_detection::detector::stages::detect::detect_stars_test;
use crate::star_detection::region::Region;
use crate::testing::synthetic::stamps::benchmark_star_field;

/// Create a synthetic Gaussian star for benchmarking.
fn make_gaussian_star(
    width: usize,
    height: usize,
    pos: Vec2,
    sigma: f32,
    amplitude: f32,
    background: f32,
) -> Buffer2<f32> {
    let mut pixels = vec![background; width * height];
    for y in 0..height {
        for x in 0..width {
            let pixel_pos = Vec2::new(x as f32, y as f32);
            let r2 = pixel_pos.distance_squared(pos);
            let value = amplitude * (-r2 / (2.0 * sigma * sigma)).exp();
            if value > 0.001 {
                pixels[y * width + x] += value;
            }
        }
    }
    Buffer2::new(width, height, pixels)
}

// =============================================================================
// Single Star Centroid Benchmarks
// =============================================================================

#[quick_bench(warmup_iters = 10, iters = 2000)]
fn bench_measure_star_single(b: ::bench::Bencher) {
    // Single star centroid computation with WeightedMoments
    let width = 64;
    let height = 64;
    let pixels = make_gaussian_star(width, height, Vec2::new(32.3, 32.7), 2.5, 0.8, 0.1);
    let bg = crate::testing::estimate_background(&pixels, &Config::default());
    let candidates = detect_stars_test(&pixels, None, &bg, &Config::default());
    let region = candidates.first().expect("Should detect star");
    let config = Config {
        centroid_method: CentroidMethod::WeightedMoments,
        ..Default::default()
    };

    b.bench(|| {
        black_box(measure_star(
            black_box(&pixels),
            black_box(&bg),
            black_box(region),
            black_box(&config),
        ))
    });
}

#[quick_bench(warmup_iters = 10, iters = 2000)]
fn bench_measure_star_gaussian_fit(b: ::bench::Bencher) {
    // Single star centroid with Gaussian fitting
    let width = 64;
    let height = 64;
    let pixels = make_gaussian_star(width, height, Vec2::new(32.3, 32.7), 2.5, 0.8, 0.1);
    let bg = crate::testing::estimate_background(&pixels, &Config::default());
    let candidates = detect_stars_test(&pixels, None, &bg, &Config::default());
    let region = candidates.first().expect("Should detect star");
    let config = Config {
        centroid_method: CentroidMethod::GaussianFit,
        ..Default::default()
    };

    b.bench(|| {
        black_box(measure_star(
            black_box(&pixels),
            black_box(&bg),
            black_box(region),
            black_box(&config),
        ))
    });
}

#[quick_bench(warmup_iters = 10, iters = 2000)]
fn bench_measure_star_moffat_fit(b: ::bench::Bencher) {
    // Single star centroid with Moffat fitting
    let width = 64;
    let height = 64;
    let pixels = make_gaussian_star(width, height, Vec2::new(32.3, 32.7), 2.5, 0.8, 0.1);
    let bg = crate::testing::estimate_background(&pixels, &Config::default());
    let candidates = detect_stars_test(&pixels, None, &bg, &Config::default());
    let region = candidates.first().expect("Should detect star");
    let config = Config {
        centroid_method: CentroidMethod::MoffatFit { beta: 2.5 },
        ..Default::default()
    };

    b.bench(|| {
        black_box(measure_star(
            black_box(&pixels),
            black_box(&bg),
            black_box(region),
            black_box(&config),
        ))
    });
}

#[quick_bench(warmup_iters = 10, iters = 2000)]
fn bench_measure_star_local_annulus(b: ::bench::Bencher) {
    // Single star centroid with LocalAnnulus background
    let width = 128;
    let height = 128;
    let pixels = make_gaussian_star(width, height, Vec2::splat(64.0), 2.5, 0.8, 0.1);
    let bg = crate::testing::estimate_background(&pixels, &Config::default());
    let candidates = detect_stars_test(&pixels, None, &bg, &Config::default());
    let region = candidates.first().expect("Should detect star");
    let config = Config {
        centroid_method: CentroidMethod::WeightedMoments,
        local_background: LocalBackgroundMethod::LocalAnnulus,
        ..Default::default()
    };

    b.bench(|| {
        black_box(measure_star(
            black_box(&pixels),
            black_box(&bg),
            black_box(region),
            black_box(&config),
        ))
    });
}

// =============================================================================
// Batch Processing Benchmarks
// =============================================================================

#[quick_bench(warmup_iters = 5, iters = 200)]
fn bench_measure_star_batch_100(b: ::bench::Bencher) {
    // 100 stars batch processing with WeightedMoments
    let pixels = benchmark_star_field(512, 512, 100, 0.1, 0.01, 42);
    let bg = crate::testing::estimate_background(&pixels, &Config::default());
    let candidates = detect_stars_test(&pixels, None, &bg, &Config::default());
    let regions: Vec<_> = candidates.iter().collect();
    let config = Config {
        centroid_method: CentroidMethod::WeightedMoments,
        ..Default::default()
    };

    b.bench(|| {
        let stars: Vec<_> = regions
            .iter()
            .filter_map(|r| measure_star(&pixels, &bg, r, &config))
            .collect();
        black_box(stars)
    });
}

#[quick_bench(warmup_iters = 2, iters = 10)]
fn bench_measure_star_batch_6k_10000(b: ::bench::Bencher) {
    // 2000 stars on 4K image - compare all centroid methods
    let pixels = benchmark_star_field(6144, 6144, 10000, 0.1, 0.01, 42);
    let bg = crate::testing::estimate_background(&pixels, &Config::default());
    let candidates = detect_stars_test(&pixels, None, &bg, &Config::default());
    let regions: Vec<_> = candidates.iter().collect();

    let config_moments = Config {
        centroid_method: CentroidMethod::WeightedMoments,
        ..Default::default()
    };
    let config_gaussian = Config {
        centroid_method: CentroidMethod::GaussianFit,
        ..Default::default()
    };
    let config_moffat = Config {
        centroid_method: CentroidMethod::MoffatFit { beta: 2.5 },
        ..Default::default()
    };

    b.bench_labeled("weighted_moments", || {
        let stars: Vec<_> = regions
            .iter()
            .filter_map(|r| measure_star(&pixels, &bg, r, &config_moments))
            .collect();
        black_box(stars)
    });

    b.bench_labeled("gaussian_fit", || {
        let stars: Vec<_> = regions
            .iter()
            .filter_map(|r| measure_star(&pixels, &bg, r, &config_gaussian))
            .collect();
        black_box(stars)
    });

    b.bench_labeled("moffat_fit", || {
        let stars: Vec<_> = regions
            .iter()
            .filter_map(|r| measure_star(&pixels, &bg, r, &config_moffat))
            .collect();
        black_box(stars)
    });
}

// =============================================================================
// refine_centroid Benchmark (isolates exp() performance)
// =============================================================================

#[quick_bench(warmup_iters = 10, iters = 10000)]
fn bench_refine_centroid_single(b: ::bench::Bencher) {
    // Single refine_centroid call - isolates the exp() hot path
    let width = 64;
    let height = 64;
    let pixels = make_gaussian_star(width, height, Vec2::new(32.3, 32.7), 2.5, 0.8, 0.1);
    let bg = crate::testing::estimate_background(&pixels, &Config::default());
    let stamp_radius = 7; // typical for FWHM ~4
    let expected_fwhm = 4.0;

    b.bench(|| {
        black_box(refine_centroid(
            black_box(&*pixels),
            black_box(width),
            black_box(height),
            black_box(&bg),
            black_box(Vec2::splat(32.0)),
            black_box(stamp_radius),
            black_box(expected_fwhm),
        ))
    });
}

#[quick_bench(warmup_iters = 10, iters = 1000)]
fn bench_refine_centroid_batch_1000(b: ::bench::Bencher) {
    // 1000 refine_centroid calls to amplify exp() cost
    let width = 64;
    let height = 64;
    let pixels = make_gaussian_star(width, height, Vec2::new(32.3, 32.7), 2.5, 0.8, 0.1);
    let bg = crate::testing::estimate_background(&pixels, &Config::default());
    let stamp_radius = 7;
    let expected_fwhm = 4.0;

    b.bench(|| {
        for _ in 0..1000 {
            black_box(refine_centroid(
                black_box(&*pixels),
                black_box(width),
                black_box(height),
                black_box(&bg),
                black_box(Vec2::splat(32.0)),
                black_box(stamp_radius),
                black_box(expected_fwhm),
            ));
        }
    });
}

// =============================================================================
// Low-Level Fitting Benchmarks
// =============================================================================

#[quick_bench(warmup_iters = 10, iters = 2000)]
fn bench_gaussian_fit_single(b: ::bench::Bencher) {
    // Single Gaussian fit (L-M optimization only)
    let width = 21;
    let height = 21;
    let background = 0.1f32;
    let sigma = 2.5f32;
    let cx = 10.3f32;
    let cy = 10.7f32;

    let mut pixels = vec![background; width * height];
    let center = Vec2::new(cx, cy);
    for y in 0..height {
        for x in 0..width {
            let pixel_pos = Vec2::new(x as f32, y as f32);
            let r2 = pixel_pos.distance_squared(center);
            pixels[y * width + x] += 1.0 * (-0.5 * r2 / (sigma * sigma)).exp();
        }
    }
    let pixels = Buffer2::new(width, height, pixels);
    let config = GaussianFitConfig::default();

    b.bench(|| {
        black_box(fit_gaussian_2d(
            black_box(&pixels),
            black_box(Vec2::splat(10.0)),
            black_box(8),
            black_box(background),
            black_box(&config),
        ))
    });
}

#[quick_bench(warmup_iters = 10, iters = 2000)]
fn bench_moffat_fit_single(b: ::bench::Bencher) {
    // Single Moffat fit (L-M optimization only)
    let width = 21;
    let height = 21;
    let background = 0.1f32;
    let alpha = 2.5f32;
    let beta = 2.5f32;
    let cx = 10.3f32;
    let cy = 10.7f32;

    let mut pixels = vec![background; width * height];
    for y in 0..height {
        for x in 0..width {
            let r2 = (x as f32 - cx).powi(2) + (y as f32 - cy).powi(2);
            pixels[y * width + x] += 1.0 * (1.0 + r2 / (alpha * alpha)).powf(-beta);
        }
    }
    let pixels = Buffer2::new(width, height, pixels);
    let config = MoffatFitConfig {
        fit_beta: false,
        fixed_beta: beta,
        ..Default::default()
    };

    b.bench(|| {
        black_box(fit_moffat_2d(
            black_box(&pixels),
            black_box(Vec2::splat(10.0)),
            black_box(8),
            black_box(background),
            black_box(&config),
        ))
    });
}

#[quick_bench(warmup_iters = 10, iters = 2000)]
fn bench_moffat_fit_variable_beta(b: ::bench::Bencher) {
    // Single Moffat fit with variable beta (6-parameter fit)
    let width = 21;
    let height = 21;
    let background = 0.1f32;
    let alpha = 2.5f32;
    let beta = 2.5f32;
    let cx = 10.0f32;
    let cy = 10.0f32;

    let mut pixels = vec![background; width * height];
    for y in 0..height {
        for x in 0..width {
            let r2 = (x as f32 - cx).powi(2) + (y as f32 - cy).powi(2);
            pixels[y * width + x] += 1.0 * (1.0 + r2 / (alpha * alpha)).powf(-beta);
        }
    }
    let pixels = Buffer2::new(width, height, pixels);
    let config = MoffatFitConfig {
        fit_beta: true,
        fixed_beta: 2.5,
        ..Default::default()
    };

    b.bench(|| {
        black_box(fit_moffat_2d(
            black_box(&pixels),
            black_box(Vec2::splat(10.0)),
            black_box(8),
            black_box(background),
            black_box(&config),
        ))
    });
}
