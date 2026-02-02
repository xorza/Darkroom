//! Benchmarks for centroid computation.
//!
//! Run with: `cargo test -p lumos --release bench_centroid -- --ignored --nocapture`

use ::bench::quick_bench;
use std::hint::black_box;

use super::compute_centroid;
use super::gaussian_fit::{GaussianFitConfig, fit_gaussian_2d};
use super::moffat_fit::{MoffatFitConfig, fit_moffat_2d};
use crate::common::Buffer2;
use crate::star_detection::background::{BackgroundConfig, BackgroundMap};
use crate::star_detection::candidate_detection::detect_stars;
use crate::star_detection::{
    CentroidConfig, CentroidMethod, LocalBackgroundMethod, StarDetectionConfig,
};
use crate::testing::synthetic::stamps::benchmark_star_field;

/// Create a synthetic Gaussian star for benchmarking.
fn make_gaussian_star(
    width: usize,
    height: usize,
    cx: f32,
    cy: f32,
    sigma: f32,
    amplitude: f32,
    background: f32,
) -> Buffer2<f32> {
    let mut pixels = vec![background; width * height];
    for y in 0..height {
        for x in 0..width {
            let dx = x as f32 - cx;
            let dy = y as f32 - cy;
            let r2 = dx * dx + dy * dy;
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

#[quick_bench(warmup_iters = 5, iters = 100)]
fn bench_compute_centroid_single(b: ::bench::Bencher) {
    // Single star centroid computation with WeightedMoments
    let width = 64;
    let height = 64;
    let pixels = make_gaussian_star(width, height, 32.3, 32.7, 2.5, 0.8, 0.1);
    let bg = crate::testing::estimate_background(&pixels, BackgroundConfig::default());
    let candidates = detect_stars(&pixels, None, &bg, &StarDetectionConfig::default());
    let candidate = candidates.first().expect("Should detect star");
    let config = StarDetectionConfig {
        centroid: CentroidConfig {
            method: CentroidMethod::WeightedMoments,
            ..Default::default()
        },
        ..Default::default()
    };

    b.bench(|| {
        black_box(compute_centroid(
            black_box(&pixels),
            black_box(&bg),
            black_box(candidate),
            black_box(&config),
        ))
    });
}

#[quick_bench(warmup_iters = 5, iters = 100)]
fn bench_compute_centroid_gaussian_fit(b: ::bench::Bencher) {
    // Single star centroid with Gaussian fitting
    let width = 64;
    let height = 64;
    let pixels = make_gaussian_star(width, height, 32.3, 32.7, 2.5, 0.8, 0.1);
    let bg = crate::testing::estimate_background(&pixels, BackgroundConfig::default());
    let candidates = detect_stars(&pixels, None, &bg, &StarDetectionConfig::default());
    let candidate = candidates.first().expect("Should detect star");
    let config = StarDetectionConfig {
        centroid: CentroidConfig {
            method: CentroidMethod::GaussianFit,
            ..Default::default()
        },
        ..Default::default()
    };

    b.bench(|| {
        black_box(compute_centroid(
            black_box(&pixels),
            black_box(&bg),
            black_box(candidate),
            black_box(&config),
        ))
    });
}

#[quick_bench(warmup_iters = 5, iters = 100)]
fn bench_compute_centroid_moffat_fit(b: ::bench::Bencher) {
    // Single star centroid with Moffat fitting
    let width = 64;
    let height = 64;
    let pixels = make_gaussian_star(width, height, 32.3, 32.7, 2.5, 0.8, 0.1);
    let bg = crate::testing::estimate_background(&pixels, BackgroundConfig::default());
    let candidates = detect_stars(&pixels, None, &bg, &StarDetectionConfig::default());
    let candidate = candidates.first().expect("Should detect star");
    let config = StarDetectionConfig {
        centroid: CentroidConfig {
            method: CentroidMethod::MoffatFit { beta: 2.5 },
            ..Default::default()
        },
        ..Default::default()
    };

    b.bench(|| {
        black_box(compute_centroid(
            black_box(&pixels),
            black_box(&bg),
            black_box(candidate),
            black_box(&config),
        ))
    });
}

#[quick_bench(warmup_iters = 5, iters = 100)]
fn bench_compute_centroid_local_annulus(b: ::bench::Bencher) {
    // Single star centroid with LocalAnnulus background
    let width = 128;
    let height = 128;
    let pixels = make_gaussian_star(width, height, 64.0, 64.0, 2.5, 0.8, 0.1);
    let bg = crate::testing::estimate_background(&pixels, BackgroundConfig::default());
    let candidates = detect_stars(&pixels, None, &bg, &StarDetectionConfig::default());
    let candidate = candidates.first().expect("Should detect star");
    let config = StarDetectionConfig {
        centroid: CentroidConfig {
            method: CentroidMethod::WeightedMoments,
            local_background_method: LocalBackgroundMethod::LocalAnnulus,
        },
        ..Default::default()
    };

    b.bench(|| {
        black_box(compute_centroid(
            black_box(&pixels),
            black_box(&bg),
            black_box(candidate),
            black_box(&config),
        ))
    });
}

// =============================================================================
// Batch Processing Benchmarks
// =============================================================================

#[quick_bench(warmup_iters = 3, iters = 20)]
fn bench_compute_centroid_batch_100(b: ::bench::Bencher) {
    // 100 stars batch processing with WeightedMoments
    let pixels = benchmark_star_field(512, 512, 100, 0.1, 0.01, 42);
    let bg = crate::testing::estimate_background(&pixels, BackgroundConfig::default());
    let candidates = detect_stars(&pixels, None, &bg, &StarDetectionConfig::default());
    let config = StarDetectionConfig {
        centroid: CentroidConfig {
            method: CentroidMethod::WeightedMoments,
            ..Default::default()
        },
        ..Default::default()
    };

    b.bench(|| {
        let stars: Vec<_> = candidates
            .iter()
            .filter_map(|c| compute_centroid(&pixels, &bg, c, &config))
            .collect();
        black_box(stars)
    });
}

#[quick_bench(warmup_iters = 2, iters = 10)]
fn bench_compute_centroid_batch_6k_10000(b: ::bench::Bencher) {
    // 2000 stars on 4K image - compare all centroid methods
    let pixels = benchmark_star_field(6144, 6144, 10000, 0.1, 0.01, 42);
    let bg = crate::testing::estimate_background(&pixels, BackgroundConfig::default());
    let candidates = detect_stars(&pixels, None, &bg, &StarDetectionConfig::default());

    let config_moments = StarDetectionConfig {
        centroid: CentroidConfig {
            method: CentroidMethod::WeightedMoments,
            ..Default::default()
        },
        ..Default::default()
    };
    let config_gaussian = StarDetectionConfig {
        centroid: CentroidConfig {
            method: CentroidMethod::GaussianFit,
            ..Default::default()
        },
        ..Default::default()
    };
    let config_moffat = StarDetectionConfig {
        centroid: CentroidConfig {
            method: CentroidMethod::MoffatFit { beta: 2.5 },
            ..Default::default()
        },
        ..Default::default()
    };

    b.bench_labeled("weighted_moments", || {
        let stars: Vec<_> = candidates
            .iter()
            .filter_map(|c| compute_centroid(&pixels, &bg, c, &config_moments))
            .collect();
        black_box(stars)
    });

    b.bench_labeled("gaussian_fit", || {
        let stars: Vec<_> = candidates
            .iter()
            .filter_map(|c| compute_centroid(&pixels, &bg, c, &config_gaussian))
            .collect();
        black_box(stars)
    });

    b.bench_labeled("moffat_fit", || {
        let stars: Vec<_> = candidates
            .iter()
            .filter_map(|c| compute_centroid(&pixels, &bg, c, &config_moffat))
            .collect();
        black_box(stars)
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
    for y in 0..height {
        for x in 0..width {
            let dx = x as f32 - cx;
            let dy = y as f32 - cy;
            pixels[y * width + x] += 1.0 * (-0.5 * (dx * dx + dy * dy) / (sigma * sigma)).exp();
        }
    }
    let pixels = Buffer2::new(width, height, pixels);
    let config = GaussianFitConfig::default();

    b.bench(|| {
        black_box(fit_gaussian_2d(
            black_box(&pixels),
            black_box(10.0),
            black_box(10.0),
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
            black_box(10.0),
            black_box(10.0),
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
            black_box(10.0),
            black_box(10.0),
            black_box(8),
            black_box(background),
            black_box(&config),
        ))
    });
}
