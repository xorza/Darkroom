//! Benchmarks for full star detection pipeline.
//!
//! Run with: `cargo test -p lumos --release bench_star_detection -- --ignored --nocapture`

use ::bench::quick_bench;
use std::hint::black_box;

use crate::astro_image::ImageDimensions;
use crate::star_detection::config::{
    BackgroundRefinement, CentroidMethod, Config, Connectivity, LocalBackgroundMethod,
};
use crate::testing::init_tracing;
use crate::testing::synthetic::generate_globular_cluster;
use crate::{AstroImage, StarDetector};

#[quick_bench(warmup_iters = 3, iters = 10)]
fn bench_detect_6k_globular_cluster(b: ::bench::Bencher) {
    init_tracing();

    // 6K globular cluster with 50000 stars - extreme crowding
    let pixels = generate_globular_cluster(6144, 6144, 50000, 42);
    let image = AstroImage::from_pixels(
        ImageDimensions::new(pixels.width(), pixels.height(), 1),
        pixels.into_vec(),
    );

    // Fully expanded config - adjust values here to experiment
    let config = Config {
        // Background
        sigma_threshold: 4.0,
        bg_mask_dilation: 3,
        min_unmasked_fraction: 0.3,
        tile_size: 64,
        sigma_clip_iterations: 5,
        refinement: BackgroundRefinement::Iterative { iterations: 2 },
        // Detection
        connectivity: Connectivity::Four,
        // Region filtering
        min_area: 5,
        max_area: 500,
        edge_margin: 10,
        // Deblending
        deblend_min_separation: 2,
        deblend_min_prominence: 0.3,
        deblend_n_thresholds: 32,
        deblend_min_contrast: 0.005,
        // Centroid
        centroid_method: CentroidMethod::WeightedMoments,
        local_background: LocalBackgroundMethod::GlobalMap,
        // PSF
        expected_fwhm: 4.0,
        psf_axis_ratio: 1.0,
        psf_angle: 0.0,
        auto_estimate_fwhm: false,
        min_stars_for_fwhm: 10,
        fwhm_estimation_sigma_factor: 2.0,
        // Star quality filtering
        min_snr: 10.0,
        max_eccentricity: 0.6,
        max_sharpness: 0.7,
        max_roundness: 1.0,
        max_fwhm_deviation: 3.0,
        duplicate_min_separation: 8.0,
        // Other
        noise_model: None,
        defect_map: None,
    };

    let mut detector = StarDetector::from_config(config);

    b.bench(|| black_box(detector.detect(black_box(&image))));
}

#[quick_bench(warmup_iters = 1, iters = 3)]
fn bench_detect_4k_dense(b: ::bench::Bencher) {
    use crate::testing::synthetic::stamps::benchmark_star_field;

    // 4K image with 2000 stars
    let pixels = benchmark_star_field(4096, 4096, 2000, 0.1, 0.01, 42);
    let image = AstroImage::from_pixels(
        ImageDimensions::new(pixels.width(), pixels.height(), 1),
        pixels.into_vec(),
    );
    let mut detector = StarDetector::new();

    b.bench(|| black_box(detector.detect(black_box(&image))));
}

#[quick_bench(warmup_iters = 2, iters = 5)]
fn bench_detect_1k_sparse(b: ::bench::Bencher) {
    use crate::testing::synthetic::stamps::benchmark_star_field;

    // 1K image with 100 stars (sparse field)
    let pixels = benchmark_star_field(1024, 1024, 100, 0.1, 0.01, 42);
    let image = AstroImage::from_pixels(
        ImageDimensions::new(pixels.width(), pixels.height(), 1),
        pixels.into_vec(),
    );
    let mut detector = StarDetector::new();

    b.bench(|| black_box(detector.detect(black_box(&image))));
}

// ============================================================================
// Component benchmarks
// ============================================================================

/// Benchmark remove_duplicate_stars with varying star counts.
/// Simulates dense star field scenario similar to rho-opiuchi detection.
#[quick_bench(warmup_iters = 5, iters = 20)]
fn bench_remove_duplicate_stars_5000(b: ::bench::Bencher) {
    use super::stages::filter::remove_duplicate_stars;
    use crate::star_detection::Star;
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
            laplacian_snr: rng.random_range(0.0..20.0),
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
fn bench_remove_duplicate_stars_10000(b: ::bench::Bencher) {
    use super::stages::filter::remove_duplicate_stars;
    use crate::star_detection::Star;
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
            laplacian_snr: rng.random_range(0.0..20.0),
        })
        .collect();

    b.bench(|| {
        let mut stars = base_stars.clone();
        stars.sort_by(|a, b| b.flux.partial_cmp(&a.flux).unwrap());
        black_box(remove_duplicate_stars(&mut stars, 5.0))
    });
}
