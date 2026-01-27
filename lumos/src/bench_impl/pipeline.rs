//! Benchmark for full astrophotography pipeline.
//!
//! This benchmark loads calibrated lights from LUMOS_CALIBRATION_DIR/calibrated_lights,
//! runs star detection, registration, and final light stacking without saving intermediates.
//!
//! Run with: cargo bench -p lumos --features bench --bench full_pipeline

use std::hint::black_box;
use std::path::PathBuf;

use criterion::{BenchmarkId, Criterion};

use crate::AstroImage;
use crate::registration::{
    InterpolationMethod, RegistrationConfig, Registrator, warp_to_reference,
};

use crate::star_detection::{Star, StarDetectionConfig, find_stars};
use crate::testing::calibration_dir;

/// Maximum stars to use for registration.
const MAX_STARS_FOR_REGISTRATION: usize = 500;

/// Register all pipeline benchmarks with Criterion.
pub fn benchmarks(c: &mut Criterion) {
    let Some(images) = load_calibrated_lights() else {
        eprintln!("Skipping pipeline benchmarks: no calibrated lights found");
        return;
    };

    if images.len() < 2 {
        eprintln!(
            "Skipping pipeline benchmarks: need at least 2 images, found {}",
            images.len()
        );
        return;
    }

    eprintln!("Loaded {} calibrated lights for benchmarking", images.len());

    benchmark_star_detection(c, &images);
    benchmark_registration(c, &images);
    benchmark_stacking(c, &images);
    benchmark_full_pipeline(c, &images);
}

/// Load calibrated light frames.
///
/// Looks for calibrated lights in the following locations (in order):
/// 1. LUMOS_CALIBRATION_DIR/calibrated_lights
/// 2. test_output/calibrated_lights (relative to lumos crate)
fn load_calibrated_lights() -> Option<Vec<AstroImage>> {
    // Try LUMOS_CALIBRATION_DIR/calibrated_lights first
    if let Some(cal_dir) = calibration_dir() {
        let calibrated_dir = cal_dir.join("calibrated_lights");
        if calibrated_dir.exists() {
            if let Some(images) = try_load_from_dir(&calibrated_dir) {
                return Some(images);
            }
        }
    }

    // Try test_output/calibrated_lights (relative to workspace root)
    let test_output = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join("test_output")
        .join("calibrated_lights");

    if test_output.exists() {
        if let Some(images) = try_load_from_dir(&test_output) {
            return Some(images);
        }
    }

    eprintln!(
        "No calibrated lights found. Run the full_pipeline example first or set LUMOS_CALIBRATION_DIR"
    );
    None
}

fn try_load_from_dir(dir: &PathBuf) -> Option<Vec<AstroImage>> {
    // Include TIFF files in addition to RAW/FITS since calibrated outputs are often TIFF
    let extensions = [
        "raf", "cr2", "cr3", "nef", "arw", "dng", "fit", "fits", "tiff", "tif",
    ];
    let paths: Vec<PathBuf> = common::file_utils::files_with_extensions(dir, &extensions);

    if paths.is_empty() {
        eprintln!("No calibrated light images found in {}", dir.display());
        return None;
    }

    eprintln!("Loading {} images from {}", paths.len(), dir.display());

    let images: Vec<AstroImage> = paths
        .iter()
        .filter_map(|p| AstroImage::from_file(p).ok())
        .collect();

    if images.is_empty() {
        return None;
    }

    Some(images)
}

/// Benchmark star detection on all images.
fn benchmark_star_detection(c: &mut Criterion, images: &[AstroImage]) {
    let mut group = c.benchmark_group("pipeline_star_detection");

    let config = StarDetectionConfig {
        edge_margin: 20,
        min_snr: 10.0,
        max_eccentricity: 0.6,
        ..StarDetectionConfig::default()
    };

    // Benchmark detection on first image
    group.bench_function(BenchmarkId::new("single_image", images[0].width()), |b| {
        b.iter(|| {
            let result = find_stars(black_box(&images[0]), black_box(&config));
            black_box(result)
        })
    });

    // Benchmark detection on all images
    group.bench_function(BenchmarkId::new("all_images", images.len()), |b| {
        b.iter(|| {
            let stars: Vec<Vec<Star>> = images
                .iter()
                .map(|img| find_stars(black_box(img), black_box(&config)).stars)
                .collect();
            black_box(stars)
        })
    });

    group.finish();
}

/// Benchmark registration between images.
fn benchmark_registration(c: &mut Criterion, images: &[AstroImage]) {
    let mut group = c.benchmark_group("pipeline_registration");

    let detection_config = StarDetectionConfig {
        edge_margin: 20,
        min_snr: 10.0,
        max_eccentricity: 0.6,
        ..StarDetectionConfig::default()
    };

    // Pre-detect stars for registration benchmarks
    let stars: Vec<Vec<Star>> = images
        .iter()
        .map(|img| find_stars(img, &detection_config).stars)
        .collect();

    let reg_config = RegistrationConfig::builder()
        .full_homography()
        .ransac_iterations(5000)
        .ransac_threshold(1.0)
        .ransac_confidence(0.9999)
        .max_stars(MAX_STARS_FOR_REGISTRATION)
        .min_matched_stars(20)
        .max_residual(1.0)
        .triangle_tolerance(0.005)
        .refine_with_centroids(true)
        .build();

    let registrator = Registrator::new(reg_config);

    // Benchmark single registration (image 1 to image 0)
    if stars.len() >= 2 {
        group.bench_function("single_pair", |b| {
            b.iter(|| {
                let result = registrator.register_stars(black_box(&stars[0]), black_box(&stars[1]));
                black_box(result)
            })
        });
    }

    // Benchmark all registrations (all images to reference)
    group.bench_function(
        BenchmarkId::new("all_to_reference", images.len() - 1),
        |b| {
            b.iter(|| {
                let results: Vec<_> = stars
                    .iter()
                    .skip(1)
                    .map(|target_stars| {
                        registrator.register_stars(black_box(&stars[0]), black_box(target_stars))
                    })
                    .collect();
                black_box(results)
            })
        },
    );

    group.finish();
}

/// Benchmark image stacking.
fn benchmark_stacking(c: &mut Criterion, images: &[AstroImage]) {
    let mut group = c.benchmark_group("pipeline_stacking");

    // For stacking benchmark, we need paths. Since we're benchmarking in-memory,
    // we'll use mean stacking which works directly with loaded images.
    // Note: The actual ImageStack API uses paths, so we benchmark mean stacking
    // on pre-loaded data to avoid I/O in the benchmark loop.

    // Benchmark mean stacking simulation (averaging pixel arrays)
    let width = images[0].width();
    let height = images[0].height();
    let channels = images[0].channels();
    let pixel_count = width * height * channels;

    group.bench_function(BenchmarkId::new("mean_accumulate", images.len()), |b| {
        b.iter(|| {
            // Simulate mean stacking: accumulate all pixels
            let mut sum = vec![0.0f64; pixel_count];
            for img in images.iter() {
                for (i, &p) in img.pixels().iter().enumerate() {
                    sum[i] += p as f64;
                }
            }
            let count = images.len() as f64;
            let result: Vec<f32> = sum.iter().map(|s| (*s / count) as f32).collect();
            black_box(result)
        })
    });

    group.finish();
}

/// Benchmark the full pipeline: star detection + registration + warping + stacking.
fn benchmark_full_pipeline(c: &mut Criterion, images: &[AstroImage]) {
    let mut group = c.benchmark_group("pipeline_full");
    group.sample_size(10); // Reduce sample size for expensive benchmark

    let detection_config = StarDetectionConfig {
        edge_margin: 20,
        min_snr: 10.0,
        max_eccentricity: 0.6,
        ..StarDetectionConfig::default()
    };

    let reg_config = RegistrationConfig::builder()
        .full_homography()
        .ransac_iterations(5000)
        .ransac_threshold(1.0)
        .ransac_confidence(0.9999)
        .max_stars(MAX_STARS_FOR_REGISTRATION)
        .min_matched_stars(20)
        .max_residual(1.0)
        .triangle_tolerance(0.005)
        .refine_with_centroids(true)
        .build();

    group.bench_function(
        BenchmarkId::new("detect_register_stack", images.len()),
        |b| {
            b.iter(|| {
                // Step 1: Detect stars in all images
                let stars: Vec<Vec<Star>> = images
                    .iter()
                    .map(|img| find_stars(img, &detection_config).stars)
                    .collect();

                // Step 2: Register all images to reference (first image)
                let registrator = Registrator::new(reg_config.clone());
                let ref_stars = &stars[0];
                let width = images[0].width();
                let height = images[0].height();
                let channels = images[0].channels();

                let mut aligned_images: Vec<AstroImage> = Vec::with_capacity(images.len());
                aligned_images.push(images[0].clone()); // Reference frame

                for (img, target_stars) in images.iter().zip(stars.iter()).skip(1) {
                    if let Ok(result) = registrator.register_stars(ref_stars, target_stars) {
                        // Warp image to align with reference
                        let warped = warp_image(img, width, height, channels, &result.transform);
                        aligned_images.push(warped);
                    }
                }

                // Step 3: Stack aligned images (simple mean)
                let pixel_count = width * height * channels;
                let mut sum = vec![0.0f64; pixel_count];
                for img in aligned_images.iter() {
                    for (i, &p) in img.pixels().iter().enumerate() {
                        sum[i] += p as f64;
                    }
                }
                let count = aligned_images.len() as f64;
                let stacked: Vec<f32> = sum.iter().map(|s| (*s / count) as f32).collect();

                black_box(AstroImage::from_pixels(width, height, channels, stacked))
            })
        },
    );

    group.finish();
}

/// Warp an image to align with the reference frame.
fn warp_image(
    image: &AstroImage,
    width: usize,
    height: usize,
    channels: usize,
    transform: &crate::TransformMatrix,
) -> AstroImage {
    let mut warped_pixels = vec![0.0f32; width * height * channels];

    for c in 0..channels {
        // Extract channel
        let channel: Vec<f32> = image
            .pixels()
            .iter()
            .skip(c)
            .step_by(channels)
            .copied()
            .collect();

        // Warp channel
        let warped_channel = warp_to_reference(
            &channel,
            width,
            height,
            transform,
            InterpolationMethod::Bilinear, // Use bilinear for speed in benchmarks
        );

        // Interleave back
        for (i, &val) in warped_channel.iter().enumerate() {
            warped_pixels[i * channels + c] = val;
        }
    }

    AstroImage::from_pixels(width, height, channels, warped_pixels)
}
