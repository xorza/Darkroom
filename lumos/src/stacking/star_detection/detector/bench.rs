//! Benchmarks for full star detection pipeline.
//!
//! Run with: `cargo test -p lumos --release bench_star_detection -- --ignored --nocapture`

use ::quickbench::quick_bench;
use std::hint::black_box;

use crate::io::astro_image::ImageDimensions;
use crate::stacking::star_detection::config::{
    BackgroundConfig, BackgroundRefinement, CentroidMethod, Config, Connectivity, DetectionConfig,
    FilterConfig, FwhmConfig, LocalBackgroundMethod, MeasurementConfig,
};
use crate::stacking::star_detection::detector::stages::detect::test_support::collect_components;
use crate::stacking::star_detection::labeling::LabelMap;
use crate::stacking::star_detection::labeling::test_utils::label_map_from_raw;
use crate::testing::init_tracing;
use crate::testing::synthetic::fixtures::{cluster_field, star_field};
use crate::{LinearImage, StarDetector};
use imaginarium::Buffer2;

#[quick_bench(warmup_iters = 3, iters = 10)]
fn bench_detect_6k_globular_cluster(b: ::quickbench::Bencher) {
    init_tracing();

    // 6K globular cluster with 50000 stars - extreme crowding
    let pixels = cluster_field(6144, 6144, 50000, 42)
        .image
        .channel(0)
        .clone();
    let image = LinearImage::from_pixels(
        ImageDimensions::new((pixels.width(), pixels.height()), 1),
        pixels.into_vec(),
    );

    // Fully expanded config - adjust values here to experiment
    let config = Config {
        background: BackgroundConfig {
            tile_size: 64,
            sigma_clip_iterations: 5,
            refinement: BackgroundRefinement::Iterative { iterations: 2 },
            mask_dilation: 3,
        },
        detection: DetectionConfig {
            sigma_threshold: 4.0,
            connectivity: Connectivity::Eight,
            psf_axis_ratio: 1.0,
            psf_angle: 0.0,
            deblend_min_separation: 2,
            deblend_min_prominence: 0.3,
            deblend_n_thresholds: 32,
            deblend_min_contrast: 0.005,
            min_area: 5,
            max_area: 500,
            edge_margin: 10,
        },
        fwhm: FwhmConfig {
            expected: 4.0,
            auto_estimate: false,
            min_stars: 10,
            estimation_sigma_factor: 2.0,
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
            max_fwhm_deviation: 3.0,
            duplicate_min_separation: 8.0,
        },
    };

    let mut detector = StarDetector::from_config(config).unwrap();

    b.bench(|| black_box(detector.detect(black_box(&image))));
}

#[quick_bench(warmup_iters = 1, iters = 3)]
fn bench_detect_4k_dense(b: ::quickbench::Bencher) {
    // 4K image with 2000 stars
    let pixels = star_field(4096, 4096, 2000, 42).image.channel(0).clone();
    let image = LinearImage::from_pixels(
        ImageDimensions::new((pixels.width(), pixels.height()), 1),
        pixels.into_vec(),
    );
    let mut detector = StarDetector::default();

    b.bench(|| black_box(detector.detect(black_box(&image))));
}

#[quick_bench(warmup_iters = 2, iters = 5)]
fn bench_detect_1k_sparse(b: ::quickbench::Bencher) {
    // 1K image with 100 stars (sparse field)
    let pixels = star_field(1024, 1024, 100, 42).image.channel(0).clone();
    let image = LinearImage::from_pixels(
        ImageDimensions::new((pixels.width(), pixels.height()), 1),
        pixels.into_vec(),
    );
    let mut detector = StarDetector::default();

    b.bench(|| black_box(detector.detect(black_box(&image))));
}

/// Benchmark remove_duplicate_stars with varying star counts.
/// Simulates dense star field scenario similar to rho-opiuchi detection.
#[quick_bench(warmup_iters = 5, iters = 20)]
fn bench_remove_duplicate_stars_5000(b: ::quickbench::Bencher) {
    use crate::stacking::star_detection::detector::stages::filter::remove_duplicate_stars;
    use crate::stacking::star_detection::star::Star;
    use rand::prelude::*;

    // Generate 5000 stars in a 4K image area (similar to dense field)
    let mut rng = StdRng::seed_from_u64(42);
    let base_stars: Vec<Star> = (0..5000)
        .map(|_| Star {
            pos: glam::DVec2::new(rng.random_range(0.0..4096.0), rng.random_range(0.0..4096.0)),
            flux: rng.random_range(100.0..10000.0),
            fwhm: rng.random_range(2.0..6.0),
            eccentricity: rng.random_range(0.0..0.3),
            snr: rng.random_range(10.0..100.0),
            peak: rng.random_range(0.1..0.9),
            sharpness: rng.random_range(0.2..0.5),
            roundness1: rng.random_range(-0.1..0.1),
            roundness2: rng.random_range(-0.1..0.1),
        })
        .collect();

    b.bench(|| {
        let mut stars = base_stars.clone();
        // Sort by flux (required by the algorithm)
        stars.sort_by(|a, b| b.flux.partial_cmp(&a.flux).unwrap());
        black_box(remove_duplicate_stars(&mut stars, 5.0))
    });
}

#[quick_bench(warmup_iters = 5, iters = 20)]
fn bench_remove_duplicate_stars_10000(b: ::quickbench::Bencher) {
    use crate::stacking::star_detection::detector::stages::filter::remove_duplicate_stars;
    use crate::stacking::star_detection::star::Star;
    use rand::prelude::*;

    // Generate 10000 stars - extreme case
    let mut rng = StdRng::seed_from_u64(42);
    let base_stars: Vec<Star> = (0..10000)
        .map(|_| Star {
            pos: glam::DVec2::new(rng.random_range(0.0..8000.0), rng.random_range(0.0..6000.0)),
            flux: rng.random_range(100.0..10000.0),
            fwhm: rng.random_range(2.0..6.0),
            eccentricity: rng.random_range(0.0..0.3),
            snr: rng.random_range(10.0..100.0),
            peak: rng.random_range(0.1..0.9),
            sharpness: rng.random_range(0.2..0.5),
            roundness1: rng.random_range(-0.1..0.1),
            roundness2: rng.random_range(-0.1..0.1),
        })
        .collect();

    b.bench(|| {
        let mut stars = base_stars.clone();
        stars.sort_by(|a, b| b.flux.partial_cmp(&a.flux).unwrap());
        black_box(remove_duplicate_stars(&mut stars, 5.0))
    });
}

fn component_label_map(width: usize, height: usize, components: usize) -> LabelMap {
    let mut labels = Buffer2::new_filled(width, height, 0u32);
    let columns = width / 4;
    let capacity = columns * (height / 4);
    assert!((1..=capacity).contains(&components));
    for component in 0..components {
        let slot = component * capacity / components;
        let base_x = slot % columns * 4;
        let base_y = slot / columns * 4;
        for y in base_y..base_y + 3 {
            for x in base_x..base_x + 3 {
                labels[(x, y)] = (component + 1) as u32;
            }
        }
    }
    label_map_from_raw(labels, components)
}

#[quick_bench(warmup_iters = 2, iters = 10)]
fn bench_components_4k_sparse(b: ::quickbench::Bencher) {
    let labels = component_label_map(4096, 4096, 2_000);
    b.bench(|| black_box(collect_components(black_box(&labels))));
}

#[quick_bench(warmup_iters = 2, iters = 10)]
fn bench_components_6k_crowded(b: ::quickbench::Bencher) {
    let labels = component_label_map(6144, 6144, 50_000);
    b.bench(|| black_box(collect_components(black_box(&labels))));
}

#[quick_bench(warmup_iters = 2, iters = 10)]
fn bench_components_2k_low_threshold(b: ::quickbench::Bencher) {
    let labels = component_label_map(2048, 2048, 100_000);
    b.bench(|| black_box(collect_components(black_box(&labels))));
}

#[quick_bench(warmup_iters = 2, iters = 10)]
fn bench_components_4k_crossover(b: ::quickbench::Bencher) {
    for components in [100, 500, 2_000, 10_000, 25_000, 50_000, 100_000] {
        let labels = component_label_map(4096, 4096, components);
        b.bench_labeled(&components.to_string(), || {
            black_box(collect_components(black_box(&labels)))
        });
    }
}
