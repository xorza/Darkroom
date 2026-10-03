//! Benchmarks for full star detection pipeline.
//!
//! Run with: `cargo test -p lumos --release --features bench bench_star_detection -- --ignored
//! --nocapture`

use crate::internals::prelude::*;
use ::quickbench::quick_bench;
use std::hint::black_box;

use crate::StarDetector;
use crate::internals::init_tracing;
use crate::internals::synthetic::fixtures::{cluster_field, star_field};
use crate::star_detection::config::Config;
use crate::star_detection::config::background_config::{BackgroundConfig, BackgroundRefinement};
use crate::star_detection::config::detection_config::{Connectivity, Deblend, DetectionConfig};
use crate::star_detection::config::filter_config::FilterConfig;
use crate::star_detection::config::fwhm_config::{FwhmConfig, FwhmMode};
use crate::star_detection::config::measurement_config::{
    CentroidMethod, LocalBackgroundMethod, MeasurementConfig,
};

#[quick_bench(warmup_time_ms = 200, bench_time_ms = 1000)]
fn bench_detect_6k_globular_cluster(b: ::quickbench::Bencher) {
    init_tracing();

    // 6K globular cluster with 50000 stars - extreme crowding
    let image = cluster_field(Size2us::new(6144, 6144), 50000, 42).image;

    // Fully expanded config - adjust values here to experiment
    let config = Config {
        background: BackgroundConfig {
            tile_size: 64,
            sigma_clip_iterations: 5,
            refinement: BackgroundRefinement::Iterative {
                iterations: 2,
                mask_dilation: 3,
                mask_sigma: 2.0,
            },
        },
        detection: DetectionConfig {
            sigma_threshold: 4.0,
            connectivity: Connectivity::Eight,
            deblend: Deblend::MultiThreshold {
                n_thresholds: 32,
                min_contrast: 0.005,
            },
            deblend_min_separation: 2,
            min_area: 5,
            max_area: 500,
            edge_margin: 10,
        },
        fwhm: FwhmConfig {
            mode: Some(FwhmMode::Fixed(4.0)),
            min_stars: 10,
            estimation_sigma_factor: 2.0,
            psf_axis_ratio: 1.0,
            psf_angle: 0.0,
        },
        measurement: MeasurementConfig {
            centroid_method: CentroidMethod::WeightedMoments,
            local_background: LocalBackgroundMethod::GlobalMap,
            noise_model: None,
        },
        filter: FilterConfig {
            min_snr: 10.0,
            max_eccentricity: 0.6,
            max_sharpness: 0.7,
            max_roundness: 1.0,
            max_fwhm_deviation: Some(3.0),
            duplicate_min_separation: 8.0,
        },
    };

    let mut detector = StarDetector::from_config(config).unwrap();

    b.bench(|| black_box(detector.detect(black_box(&image))));
}

#[quick_bench(warmup_time_ms = 200, bench_time_ms = 1000)]
fn bench_detect_4k_dense(b: ::quickbench::Bencher) {
    // 4K image with 2000 stars
    let image = star_field(Size2us::new(4096, 4096), 2000, 42).image;
    let mut detector = StarDetector::default();

    b.bench(|| black_box(detector.detect(black_box(&image))));
}

#[quick_bench(warmup_time_ms = 200, bench_time_ms = 1000)]
fn bench_detect_1k_sparse(b: ::quickbench::Bencher) {
    // 1K image with 100 stars (sparse field)
    let image = star_field(Size2us::new(1024, 1024), 100, 42).image;
    let mut detector = StarDetector::default();

    b.bench(|| black_box(detector.detect(black_box(&image))));
}
