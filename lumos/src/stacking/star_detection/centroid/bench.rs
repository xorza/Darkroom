//! Benchmarks for centroid computation.
//!
//! Run with: `cargo test -p lumos --release bench_centroid -- --ignored --nocapture`
use crate::stacking::star_detection::centroid::{StampGrid, compute_stamp_radius};
use crate::testing::prelude::*;

use ::quickbench::quick_bench;
use std::hint::black_box;

use crate::stacking::star_detection::centroid::gaussian_fit::{GaussianFit, GaussianFitConfig};
use crate::stacking::star_detection::centroid::measure_star;
use crate::stacking::star_detection::centroid::moffat_fit::{MoffatFit, MoffatFitConfig};
use crate::stacking::star_detection::centroid::refine_centroid;
use crate::stacking::star_detection::config::background_config::BackgroundConfig;
use crate::stacking::star_detection::config::detection_config::DetectionConfig;
use crate::stacking::star_detection::config::measurement_config::{
    CentroidMethod, LocalBackgroundMethod, MeasurementConfig,
};
use crate::stacking::star_detection::detector::stages::detect::internals::detect_stars_test;
use crate::testing::synthetic::fixtures::star_field;
use crate::testing::synthetic::star_profiles::{StarProfile, SyntheticStar};

#[quick_bench(warmup_iters = 100, iters = 10000)]
fn bench_measure_star_single(b: ::quickbench::Bencher) {
    // Single star centroid computation with WeightedMoments
    let width = 64;
    let height = 64;
    let pixels = SyntheticStar::new(
        Vec2::new(32.3, 32.7),
        0.8,
        StarProfile::Gaussian { sigma: 2.5 },
    )
    .stamp(Size2us::new(width, height), 0.1);
    let bg = background_map::estimate(&pixels, &BackgroundConfig::default());
    let candidates = detect_stars_test(&pixels, &bg, &DetectionConfig::default());
    let region = candidates.first().expect("Should detect star");
    let config = MeasurementConfig {
        centroid_method: CentroidMethod::WeightedMoments,
        ..Default::default()
    };

    b.bench(|| {
        black_box(measure_star(
            black_box(&pixels),
            black_box(&bg),
            black_box(region),
            black_box(&config),
            4.0,
            black_box(&StampGrid::new(compute_stamp_radius(4.0))),
        ))
    });
}

#[quick_bench(warmup_iters = 100, iters = 10000)]
fn bench_measure_star_gaussian_fit(b: ::quickbench::Bencher) {
    // Single star centroid with Gaussian fitting
    let width = 64;
    let height = 64;
    let pixels = SyntheticStar::new(
        Vec2::new(32.3, 32.7),
        0.8,
        StarProfile::Gaussian { sigma: 2.5 },
    )
    .stamp(Size2us::new(width, height), 0.1);
    let bg = background_map::estimate(&pixels, &BackgroundConfig::default());
    let candidates = detect_stars_test(&pixels, &bg, &DetectionConfig::default());
    let region = candidates.first().expect("Should detect star");
    let config = MeasurementConfig {
        centroid_method: CentroidMethod::GaussianFit,
        ..Default::default()
    };

    b.bench(|| {
        black_box(measure_star(
            black_box(&pixels),
            black_box(&bg),
            black_box(region),
            black_box(&config),
            4.0,
            black_box(&StampGrid::new(compute_stamp_radius(4.0))),
        ))
    });
}

#[quick_bench(warmup_iters = 100, iters = 10000)]
fn bench_measure_star_moffat_fit(b: ::quickbench::Bencher) {
    // Single star centroid with Moffat fitting
    let width = 64;
    let height = 64;
    let pixels = SyntheticStar::new(
        Vec2::new(32.3, 32.7),
        0.8,
        StarProfile::Gaussian { sigma: 2.5 },
    )
    .stamp(Size2us::new(width, height), 0.1);
    let bg = background_map::estimate(&pixels, &BackgroundConfig::default());
    let candidates = detect_stars_test(&pixels, &bg, &DetectionConfig::default());
    let region = candidates.first().expect("Should detect star");
    let config = MeasurementConfig {
        centroid_method: CentroidMethod::MoffatFit { beta: 2.5 },
        ..Default::default()
    };

    b.bench(|| {
        black_box(measure_star(
            black_box(&pixels),
            black_box(&bg),
            black_box(region),
            black_box(&config),
            4.0,
            black_box(&StampGrid::new(compute_stamp_radius(4.0))),
        ))
    });
}

#[quick_bench(warmup_iters = 100, iters = 10000)]
fn bench_measure_star_local_annulus(b: ::quickbench::Bencher) {
    // Single star centroid with LocalAnnulus background
    let width = 128;
    let height = 128;
    let pixels = SyntheticStar::new(Vec2::splat(64.0), 0.8, StarProfile::Gaussian { sigma: 2.5 })
        .stamp(Size2us::new(width, height), 0.1);
    let bg = background_map::estimate(&pixels, &BackgroundConfig::default());
    let candidates = detect_stars_test(&pixels, &bg, &DetectionConfig::default());
    let region = candidates.first().expect("Should detect star");
    let config = MeasurementConfig {
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
            4.0,
            black_box(&StampGrid::new(compute_stamp_radius(4.0))),
        ))
    });
}

#[quick_bench(warmup_iters = 5, iters = 200)]
fn bench_measure_star_batch_100(b: ::quickbench::Bencher) {
    // 100 stars batch processing with WeightedMoments
    let pixels = star_field(Size2us::new(512, 512), 100, 42)
        .image
        .channel(0)
        .clone();
    let bg = background_map::estimate(&pixels, &BackgroundConfig::default());
    let candidates = detect_stars_test(&pixels, &bg, &DetectionConfig::default());
    let regions: Vec<_> = candidates.iter().collect();
    let config = MeasurementConfig {
        centroid_method: CentroidMethod::WeightedMoments,
        ..Default::default()
    };

    let grid = StampGrid::new(compute_stamp_radius(4.0));
    b.bench(|| {
        let stars: Vec<_> = regions
            .iter()
            .filter_map(|r| measure_star(&pixels, &bg, r, &config, 4.0, &grid))
            .collect();
        black_box(stars)
    });
}

#[quick_bench(warmup_iters = 2, iters = 10)]
fn bench_measure_star_batch_6k_10000(b: ::quickbench::Bencher) {
    // 2000 stars on 4K image - compare all centroid methods
    let pixels = star_field(Size2us::new(6144, 6144), 10000, 42)
        .image
        .channel(0)
        .clone();
    let bg = background_map::estimate(&pixels, &BackgroundConfig::default());
    let candidates = detect_stars_test(&pixels, &bg, &DetectionConfig::default());
    let regions: Vec<_> = candidates.iter().collect();

    let config_moments = MeasurementConfig {
        centroid_method: CentroidMethod::WeightedMoments,
        ..Default::default()
    };
    let config_gaussian = MeasurementConfig {
        centroid_method: CentroidMethod::GaussianFit,
        ..Default::default()
    };
    let config_moffat = MeasurementConfig {
        centroid_method: CentroidMethod::MoffatFit { beta: 2.5 },
        ..Default::default()
    };

    let grid = StampGrid::new(compute_stamp_radius(4.0));

    b.bench_labeled("weighted_moments", || {
        let stars: Vec<_> = regions
            .iter()
            .filter_map(|r| measure_star(&pixels, &bg, r, &config_moments, 4.0, &grid))
            .collect();
        black_box(stars)
    });

    b.bench_labeled("gaussian_fit", || {
        let stars: Vec<_> = regions
            .iter()
            .filter_map(|r| measure_star(&pixels, &bg, r, &config_gaussian, 4.0, &grid))
            .collect();
        black_box(stars)
    });

    b.bench_labeled("moffat_fit", || {
        let stars: Vec<_> = regions
            .iter()
            .filter_map(|r| measure_star(&pixels, &bg, r, &config_moffat, 4.0, &grid))
            .collect();
        black_box(stars)
    });
}

#[quick_bench(warmup_iters = 10, iters = 10000)]
fn bench_refine_centroid_single(b: ::quickbench::Bencher) {
    // Single refine_centroid call - isolates the exp() hot path
    let width = 64;
    let height = 64;
    let pixels = SyntheticStar::new(
        Vec2::new(32.3, 32.7),
        0.8,
        StarProfile::Gaussian { sigma: 2.5 },
    )
    .stamp(Size2us::new(width, height), 0.1);
    let bg = background_map::estimate(&pixels, &BackgroundConfig::default());
    let stamp_radius = 7; // typical for FWHM ~4
    let expected_fwhm = 4.0;

    b.bench(|| {
        black_box(refine_centroid(
            black_box(&pixels),
            black_box(&bg),
            black_box(DVec2::splat(32.0)),
            black_box(stamp_radius),
            black_box(expected_fwhm),
        ))
    });
}

#[quick_bench(warmup_iters = 10, iters = 1000)]
fn bench_refine_centroid_batch_1000(b: ::quickbench::Bencher) {
    // 1000 refine_centroid calls to amplify exp() cost
    let width = 64;
    let height = 64;
    let pixels = SyntheticStar::new(
        Vec2::new(32.3, 32.7),
        0.8,
        StarProfile::Gaussian { sigma: 2.5 },
    )
    .stamp(Size2us::new(width, height), 0.1);
    let bg = background_map::estimate(&pixels, &BackgroundConfig::default());
    let stamp_radius = 7;
    let expected_fwhm = 4.0;

    b.bench(|| {
        for _ in 0..1000 {
            black_box(refine_centroid(
                black_box(&pixels),
                black_box(&bg),
                black_box(DVec2::splat(32.0)),
                black_box(stamp_radius),
                black_box(expected_fwhm),
            ));
        }
    });
}

#[quick_bench(warmup_iters = 100, iters = 10000)]
fn bench_gaussian_fit_single(b: ::quickbench::Bencher) {
    // Single Gaussian fit (L-M optimization only)
    let width = 21;
    let height = 21;
    let background = 0.1f32;
    let sigma = 2.5f32;
    let cx = 10.3f32;
    let cy = 10.7f32;

    let pixels = SyntheticStar::new(Vec2::new(cx, cy), 1.0, StarProfile::Gaussian { sigma })
        .stamp(Size2us::new(width, height), background);
    let config = GaussianFitConfig::default();

    b.bench(|| {
        black_box(GaussianFit::new(
            black_box(&pixels),
            black_box(DVec2::splat(10.0)),
            black_box(&StampGrid::new(8)),
            black_box(background),
            None,
            black_box(&config),
        ))
    });
}

#[quick_bench(warmup_iters = 100, iters = 10000)]
fn bench_moffat_fit_single(b: ::quickbench::Bencher) {
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
        fixed_beta: beta,
        ..Default::default()
    };

    b.bench(|| {
        black_box(MoffatFit::new(
            black_box(&pixels),
            black_box(DVec2::splat(10.0)),
            black_box(&StampGrid::new(8)),
            black_box(background),
            None,
            black_box(&config),
        ))
    });
}
