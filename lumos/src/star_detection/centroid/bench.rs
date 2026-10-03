//! Benchmarks for centroid computation.
//!
//! Run with: `cargo test -p lumos --release --features bench bench_centroid -- --ignored
//! --nocapture`
use crate::bit_buffer2::BitBuffer2;
use crate::internals::prelude::*;
use crate::internals::synthetic::background_map;
use crate::star_detection::centroid::compute_stamp_radius;
use crate::star_detection::centroid::stamp::StampGrid;

use ::quickbench::quick_bench;
use std::hint::black_box;

use crate::internals::synthetic::fixtures::star_field;
use crate::internals::synthetic::star_profiles::{StarProfile, SyntheticStar};
use crate::star_detection::background::sky_noise::SkyNoise;
use crate::star_detection::centroid::compute_star;
use crate::star_detection::centroid::covariance::windowed_covariance;
use crate::star_detection::centroid::measure_star;
use crate::star_detection::centroid::refine_centroid;
use crate::star_detection::config::background_config::BackgroundConfig;
use crate::star_detection::config::detection_config::DetectionConfig;
use crate::star_detection::config::measurement_config::{
    CentroidMethod, LocalBackgroundMethod, MeasurementConfig,
};
use crate::star_detection::detector::stages::detect::internals::detect_stars_test;

/// One Gaussian star at (32.3, 32.7) on a 64×64 field, or centred on a larger one, less its
/// estimated sky: the fixture every single-star bench below shares.
#[derive(Debug)]
struct SingleStar {
    residual: Buffer2<f32>,
    sky: SkyNoise,
    saturation: BitBuffer2,
}

impl SingleStar {
    fn at(size: Size2us, pos: Vec2) -> SingleStar {
        let pixels =
            SyntheticStar::new(pos, 0.8, StarProfile::Gaussian { sigma: 2.5 }).stamp(size, 0.1);
        let bg = background_map::estimate(&pixels, &BackgroundConfig::default());
        SingleStar {
            residual: bg.residual_of(&pixels),
            sky: bg.sky_noise(),
            saturation: BitBuffer2::new_filled(size, false),
        }
    }

    fn field() -> SingleStar {
        SingleStar::at(Size2us::new(64, 64), Vec2::new(32.3, 32.7))
    }
}

/// `measure_star` on one detected star under `config`.
fn bench_measure_star(b: ::quickbench::Bencher, star: &SingleStar, config: &MeasurementConfig) {
    let candidates = detect_stars_test(&star.residual, &star.sky, &DetectionConfig::default());
    let region = candidates.first().expect("Should detect star");
    let grid = StampGrid::new(compute_stamp_radius(4.0));
    b.bench(|| {
        black_box(measure_star(
            black_box(&star.residual),
            black_box(&star.sky),
            &star.saturation,
            black_box(region),
            black_box(config),
            4.0,
            black_box(&grid),
        ))
    });
}

fn centroid_config(centroid_method: CentroidMethod) -> MeasurementConfig {
    MeasurementConfig {
        centroid_method,
        ..Default::default()
    }
}

#[quick_bench(warmup_time_ms = 100, bench_time_ms = 500)]
fn bench_measure_star_single(b: ::quickbench::Bencher) {
    bench_measure_star(
        b,
        &SingleStar::field(),
        &centroid_config(CentroidMethod::WeightedMoments),
    );
}

#[quick_bench(warmup_time_ms = 100, bench_time_ms = 500)]
fn bench_measure_star_gaussian_fit(b: ::quickbench::Bencher) {
    bench_measure_star(
        b,
        &SingleStar::field(),
        &centroid_config(CentroidMethod::GaussianFit),
    );
}

#[quick_bench(warmup_time_ms = 100, bench_time_ms = 500)]
fn bench_measure_star_moffat_fit(b: ::quickbench::Bencher) {
    bench_measure_star(
        b,
        &SingleStar::field(),
        &centroid_config(CentroidMethod::MoffatFit { beta: 2.5 }),
    );
}

#[quick_bench(warmup_time_ms = 100, bench_time_ms = 500)]
fn bench_measure_star_local_annulus(b: ::quickbench::Bencher) {
    let config = MeasurementConfig {
        local_background: LocalBackgroundMethod::LocalAnnulus,
        ..centroid_config(CentroidMethod::WeightedMoments)
    };
    bench_measure_star(
        b,
        &SingleStar::at(Size2us::new(128, 128), Vec2::splat(64.0)),
        &config,
    );
}

#[quick_bench(warmup_time_ms = 100, bench_time_ms = 500)]
fn bench_measure_star_batch_100(b: ::quickbench::Bencher) {
    // 100 stars batch processing with WeightedMoments
    let pixels = star_field(Size2us::new(512, 512), 100, 42)
        .image
        .channel(0)
        .clone();
    let bg = background_map::estimate(&pixels, &BackgroundConfig::default());
    let residual = bg.residual_of(&pixels);
    let sky = bg.sky_noise();
    let saturation = BitBuffer2::new_filled(Size2us::new(pixels.width(), pixels.height()), false);
    let candidates = detect_stars_test(
        &bg.residual_of(&pixels),
        &bg.sky_noise(),
        &DetectionConfig::default(),
    );
    let regions: Vec<_> = candidates.iter().collect();
    let config = MeasurementConfig {
        centroid_method: CentroidMethod::WeightedMoments,
        ..Default::default()
    };

    let grid = StampGrid::new(compute_stamp_radius(4.0));
    b.bench(|| {
        let stars: Vec<_> = regions
            .iter()
            .filter_map(|r| measure_star(&residual, &sky, &saturation, r, &config, 4.0, &grid))
            .collect();
        black_box(stars)
    });
}

#[quick_bench(warmup_time_ms = 200, bench_time_ms = 1000)]
fn bench_measure_star_batch_6k_10000(b: ::quickbench::Bencher) {
    // 2000 stars on 4K image - compare all centroid methods
    let pixels = star_field(Size2us::new(6144, 6144), 10000, 42)
        .image
        .channel(0)
        .clone();
    let bg = background_map::estimate(&pixels, &BackgroundConfig::default());
    let residual = bg.residual_of(&pixels);
    let sky = bg.sky_noise();
    let saturation = BitBuffer2::new_filled(Size2us::new(pixels.width(), pixels.height()), false);
    let candidates = detect_stars_test(
        &bg.residual_of(&pixels),
        &bg.sky_noise(),
        &DetectionConfig::default(),
    );
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
            .filter_map(|r| {
                measure_star(&residual, &sky, &saturation, r, &config_moments, 4.0, &grid)
            })
            .collect();
        black_box(stars)
    });

    b.bench_labeled("gaussian_fit", || {
        let stars: Vec<_> = regions
            .iter()
            .filter_map(|r| {
                measure_star(
                    &residual,
                    &sky,
                    &saturation,
                    r,
                    &config_gaussian,
                    4.0,
                    &grid,
                )
            })
            .collect();
        black_box(stars)
    });

    b.bench_labeled("moffat_fit", || {
        let stars: Vec<_> = regions
            .iter()
            .filter_map(|r| {
                measure_star(&residual, &sky, &saturation, r, &config_moffat, 4.0, &grid)
            })
            .collect();
        black_box(stars)
    });
}

#[quick_bench(warmup_time_ms = 100, bench_time_ms = 500)]
fn bench_refine_centroid_single(b: ::quickbench::Bencher) {
    // Single refine_centroid call - isolates the exp() hot path
    let star = SingleStar::field();
    b.bench(|| {
        black_box(refine_centroid(
            black_box(&star.residual),
            black_box(DVec2::splat(32.0)),
            black_box(7),
            black_box(4.0),
        ))
    });
}

#[quick_bench(warmup_time_ms = 100, bench_time_ms = 500)]
fn bench_refine_centroid_batch_1000(b: ::quickbench::Bencher) {
    // 1000 refine_centroid calls to amplify exp() cost
    let star = SingleStar::field();
    b.bench(|| {
        for _ in 0..1000 {
            black_box(refine_centroid(
                black_box(&star.residual),
                black_box(DVec2::splat(32.0)),
                black_box(7),
                black_box(4.0),
            ));
        }
    });
}

#[quick_bench(warmup_time_ms = 100, bench_time_ms = 500)]
fn bench_compute_star_single(b: ::quickbench::Bencher) {
    // Flux, SNR, sharpness, roundness and the windowed covariance for one candidate — everything
    // `measure_star` does after the centroid is settled.
    let SingleStar { residual, sky, .. } = SingleStar::field();
    let pos = DVec2::new(32.3, 32.7);
    let peak = residual[(32, 33)];

    b.bench(|| {
        black_box(compute_star(
            black_box(&residual),
            black_box(&sky),
            black_box(pos),
            black_box(peak),
            black_box(7),
            None,
            None,
        ))
    });
}

#[quick_bench(warmup_time_ms = 200, bench_time_ms = 1000)]
fn bench_compute_star_batch_1000(b: ::quickbench::Bencher) {
    let SingleStar { residual, sky, .. } = SingleStar::field();
    let pos = DVec2::new(32.3, 32.7);
    let peak = residual[(32, 33)];

    b.bench(|| {
        for _ in 0..1000 {
            black_box(compute_star(
                black_box(&residual),
                black_box(&sky),
                black_box(pos),
                black_box(peak),
                black_box(7),
                None,
                None,
            ));
        }
    });
}

#[quick_bench(warmup_time_ms = 100, bench_time_ms = 500)]
fn bench_windowed_covariance_single(b: ::quickbench::Bencher) {
    // The adaptive-window moment loop nested inside `compute_star`: up to four re-reads of the
    // stamp's image and background rows, one per window iteration.
    let SingleStar { residual, .. } = SingleStar::field();
    // Seeded as `compute_star` seeds it — sigma 2.5 gives sigma^2 = 6.25.
    let seed_sigma_sq = 6.25;

    b.bench(|| {
        black_box(windowed_covariance(
            black_box(&residual),
            0.0,
            black_box(DVec2::new(32.3, 32.7)),
            black_box(7),
            black_box(seed_sigma_sq),
        ))
    });
}

#[quick_bench(warmup_time_ms = 100, bench_time_ms = 500)]
fn bench_windowed_covariance_batch_1000(b: ::quickbench::Bencher) {
    let SingleStar { residual, .. } = SingleStar::field();
    let seed_sigma_sq = 6.25;

    b.bench(|| {
        for _ in 0..1000 {
            black_box(windowed_covariance(
                black_box(&residual),
                0.0,
                black_box(DVec2::new(32.3, 32.7)),
                black_box(7),
                black_box(seed_sigma_sq),
            ));
        }
    });
}
