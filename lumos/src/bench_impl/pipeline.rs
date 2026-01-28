//! Benchmark for full astrophotography pipeline.
//!
//! This benchmark loads calibrated lights from LUMOS_CALIBRATION_DIR/calibrated_lights,
//! runs star detection, registration, and final light stacking without saving intermediates.
//!
//! Run with: cargo bench -p lumos --features bench --bench full_pipeline
//!
//! Individual stages can be run with:
//!   cargo bench -p lumos --features bench --bench full_pipeline -- star_detection
//!   cargo bench -p lumos --features bench --bench full_pipeline -- registration
//!   cargo bench -p lumos --features bench --bench full_pipeline -- warping
//!   cargo bench -p lumos --features bench --bench full_pipeline -- stacking
//!   cargo bench -p lumos --features bench --bench full_pipeline -- full

use std::hint::black_box;
use std::path::PathBuf;

use criterion::{BenchmarkId, Criterion};

use crate::AstroImage;
use crate::registration::{
    GpuWarper, InterpolationMethod, RegistrationConfig, Registrator, TransformMatrix,
};
use crate::star_detection::{Star, StarDetectionConfig, StarDetector};
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
    benchmark_warping(c, &images);
    benchmark_stacking(c, &images);
    benchmark_full_pipeline(c, &images);
}

/// Load calibrated light frames.
fn load_calibrated_lights() -> Option<Vec<AstroImage>> {
    // Try LUMOS_CALIBRATION_DIR/calibrated_lights first
    if let Some(cal_dir) = calibration_dir() {
        let calibrated_dir = cal_dir.join("calibrated_lights");
        if calibrated_dir.exists()
            && let Some(images) = try_load_from_dir(&calibrated_dir)
        {
            return Some(images);
        }
    }

    // Try test_output/calibrated_lights (relative to workspace root)
    let test_output = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join("test_output")
        .join("calibrated_lights");

    if test_output.exists()
        && let Some(images) = try_load_from_dir(&test_output)
    {
        return Some(images);
    }

    eprintln!(
        "No calibrated lights found. Run the full_pipeline example first or set LUMOS_CALIBRATION_DIR"
    );
    None
}

fn try_load_from_dir(dir: &std::path::Path) -> Option<Vec<AstroImage>> {
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

// =============================================================================
// Star Detection Benchmarks
// =============================================================================

fn benchmark_star_detection(c: &mut Criterion, images: &[AstroImage]) {
    let mut group = c.benchmark_group("star_detection");
    group.sample_size(10);

    let detector = StarDetector::from_config(StarDetectionConfig {
        edge_margin: 20,
        min_snr: 10.0,
        max_eccentricity: 0.6,
        ..StarDetectionConfig::default()
    });

    group.bench_function(BenchmarkId::new("single_image", images[0].width()), |b| {
        b.iter(|| {
            let result = detector.detect(black_box(&images[0]));
            black_box(result)
        })
    });

    group.bench_function(BenchmarkId::new("all_images", images.len()), |b| {
        b.iter(|| {
            let stars: Vec<Vec<Star>> = images
                .iter()
                .map(|img| detector.detect(black_box(img)).stars)
                .collect();
            black_box(stars)
        })
    });

    group.finish();
}

// =============================================================================
// Registration Benchmarks
// =============================================================================

fn benchmark_registration(c: &mut Criterion, images: &[AstroImage]) {
    let mut group = c.benchmark_group("registration");
    group.sample_size(10);

    let detector = StarDetector::from_config(StarDetectionConfig {
        edge_margin: 20,
        min_snr: 10.0,
        max_eccentricity: 0.6,
        ..StarDetectionConfig::default()
    });

    // Pre-detect stars for registration benchmarks
    let stars: Vec<Vec<Star>> = images
        .iter()
        .map(|img| detector.detect(img).stars)
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

    if stars.len() >= 2 {
        group.bench_function("single_pair", |b| {
            b.iter(|| {
                let result = registrator.register_stars(black_box(&stars[0]), black_box(&stars[1]));
                black_box(result)
            })
        });
    }

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

// =============================================================================
// Warping Benchmarks - CPU vs GPU comparison
// =============================================================================

fn benchmark_warping(c: &mut Criterion, images: &[AstroImage]) {
    let mut group = c.benchmark_group("warping");
    group.sample_size(10);

    let width = images[0].width();
    let height = images[0].height();
    let channels = images[0].channels();

    // Create a representative transform (small rotation + translation)
    let transform = TransformMatrix::similarity(10.5, -5.3, 0.01, 1.001);

    // -------------------------------------------------------------------------
    // CPU Warping
    // -------------------------------------------------------------------------
    group.bench_function("cpu_bilinear", |b| {
        b.iter(|| {
            let warped = warp_cpu(
                black_box(&images[0]),
                black_box(&transform),
                InterpolationMethod::Bilinear,
            );
            black_box(warped)
        })
    });

    group.bench_function("cpu_lanczos3", |b| {
        b.iter(|| {
            let warped = warp_cpu(
                black_box(&images[0]),
                black_box(&transform),
                InterpolationMethod::Lanczos3,
            );
            black_box(warped)
        })
    });

    // -------------------------------------------------------------------------
    // GPU Warping
    // -------------------------------------------------------------------------
    let mut gpu_warper = GpuWarper::new();

    group.bench_function("gpu_bilinear", |b| {
        b.iter(|| {
            let warped = warp_gpu(
                black_box(&mut gpu_warper),
                black_box(&images[0]),
                width,
                height,
                channels,
                black_box(&transform),
            );
            black_box(warped)
        })
    });

    // -------------------------------------------------------------------------
    // Batch warping comparison (multiple images)
    // -------------------------------------------------------------------------
    if images.len() >= 5 {
        let batch_size = 5;

        group.bench_function(BenchmarkId::new("cpu_batch", batch_size), |b| {
            b.iter(|| {
                let warped: Vec<_> = images[..batch_size]
                    .iter()
                    .map(|img| {
                        warp_cpu(
                            black_box(img),
                            black_box(&transform),
                            InterpolationMethod::Bilinear,
                        )
                    })
                    .collect();
                black_box(warped)
            })
        });

        group.bench_function(BenchmarkId::new("gpu_batch", batch_size), |b| {
            b.iter(|| {
                let warped: Vec<_> = images[..batch_size]
                    .iter()
                    .map(|img| {
                        warp_gpu(
                            black_box(&mut gpu_warper),
                            black_box(img),
                            width,
                            height,
                            channels,
                            black_box(&transform),
                        )
                    })
                    .collect();
                black_box(warped)
            })
        });
    }

    group.finish();
}

/// Warp using CPU (parallel internally via warp_to_reference_image).
fn warp_cpu(
    image: &AstroImage,
    transform: &TransformMatrix,
    method: InterpolationMethod,
) -> AstroImage {
    use crate::registration::warp_to_reference_image;

    warp_to_reference_image(image, transform, method)
}

/// Warp using GPU compute shaders.
fn warp_gpu(
    gpu_warper: &mut GpuWarper,
    image: &AstroImage,
    width: usize,
    height: usize,
    channels: usize,
    transform: &TransformMatrix,
) -> AstroImage {
    use crate::registration::warp_to_reference_image;

    let warped_pixels = if channels == 3 {
        gpu_warper.warp_rgb(image.pixels(), width, height, transform)
    } else if channels == 1 {
        gpu_warper.warp_channel(image.pixels(), width, height, transform)
    } else {
        // Fallback for other channel counts: use CPU warping
        let warped = warp_to_reference_image(image, transform, InterpolationMethod::Bilinear);
        return warped;
    };

    AstroImage::from_pixels(ImageDimensions::new(width, height, channels), warped_pixels)
}

// =============================================================================
// Stacking Benchmarks
// =============================================================================

fn benchmark_stacking(c: &mut Criterion, images: &[AstroImage]) {
    let mut group = c.benchmark_group("stacking");
    group.sample_size(10);

    let width = images[0].width();
    let height = images[0].height();
    let channels = images[0].channels();
    let pixel_count = width * height * channels;

    group.bench_function(BenchmarkId::new("mean_accumulate", images.len()), |b| {
        b.iter(|| {
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

// =============================================================================
// Full Pipeline Benchmarks - CPU vs GPU comparison
// =============================================================================

fn benchmark_full_pipeline(c: &mut Criterion, images: &[AstroImage]) {
    let mut group = c.benchmark_group("full_pipeline");
    group.sample_size(10);

    let detector = StarDetector::from_config(StarDetectionConfig {
        edge_margin: 20,
        min_snr: 10.0,
        max_eccentricity: 0.6,
        ..StarDetectionConfig::default()
    });

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

    // -------------------------------------------------------------------------
    // Full pipeline with CPU warping
    // -------------------------------------------------------------------------
    group.bench_function(BenchmarkId::new("cpu_warp", images.len()), |b| {
        b.iter(|| {
            run_full_pipeline_cpu(
                black_box(images),
                black_box(&detector),
                black_box(&reg_config),
            )
        })
    });

    // -------------------------------------------------------------------------
    // Full pipeline with GPU warping
    // -------------------------------------------------------------------------
    let mut gpu_warper = GpuWarper::new();

    group.bench_function(BenchmarkId::new("gpu_warp", images.len()), |b| {
        b.iter(|| {
            run_full_pipeline_gpu(
                black_box(images),
                black_box(&detector),
                black_box(&reg_config),
                black_box(&mut gpu_warper),
            )
        })
    });

    group.finish();
}

/// Run full pipeline with CPU warping.
fn run_full_pipeline_cpu(
    images: &[AstroImage],
    detector: &StarDetector,
    reg_config: &RegistrationConfig,
) -> AstroImage {
    // Step 1: Detect stars
    let stars: Vec<Vec<Star>> = images
        .iter()
        .map(|img| detector.detect(img).stars)
        .collect();

    // Step 2: Register and warp
    let registrator = Registrator::new(reg_config.clone());
    let ref_stars = &stars[0];
    let width = images[0].width();
    let height = images[0].height();
    let channels = images[0].channels();

    let mut aligned_images: Vec<AstroImage> = Vec::with_capacity(images.len());
    aligned_images.push(images[0].clone());

    for (img, target_stars) in images.iter().zip(stars.iter()).skip(1) {
        if let Ok(result) = registrator.register_stars(ref_stars, target_stars) {
            let warped = warp_cpu(img, &result.transform, InterpolationMethod::Bilinear);
            aligned_images.push(warped);
        }
    }

    // Step 3: Stack
    let pixel_count = width * height * channels;
    let mut sum = vec![0.0f64; pixel_count];
    for img in aligned_images.iter() {
        for (i, &p) in img.pixels().iter().enumerate() {
            sum[i] += p as f64;
        }
    }
    let count = aligned_images.len() as f64;
    let stacked: Vec<f32> = sum.iter().map(|s| (*s / count) as f32).collect();

    AstroImage::from_pixels(ImageDimensions::new(width, height, channels), stacked)
}

/// Run full pipeline with GPU warping.
fn run_full_pipeline_gpu(
    images: &[AstroImage],
    detector: &StarDetector,
    reg_config: &RegistrationConfig,
    gpu_warper: &mut GpuWarper,
) -> AstroImage {
    // Step 1: Detect stars
    let stars: Vec<Vec<Star>> = images
        .iter()
        .map(|img| detector.detect(img).stars)
        .collect();

    // Step 2: Register and warp (GPU)
    let registrator = Registrator::new(reg_config.clone());
    let ref_stars = &stars[0];
    let width = images[0].width();
    let height = images[0].height();
    let channels = images[0].channels();

    let mut aligned_images: Vec<AstroImage> = Vec::with_capacity(images.len());
    aligned_images.push(images[0].clone());

    for (img, target_stars) in images.iter().zip(stars.iter()).skip(1) {
        if let Ok(result) = registrator.register_stars(ref_stars, target_stars) {
            let warped = warp_gpu(gpu_warper, img, width, height, channels, &result.transform);
            aligned_images.push(warped);
        }
    }

    // Step 3: Stack
    let pixel_count = width * height * channels;
    let mut sum = vec![0.0f64; pixel_count];
    for img in aligned_images.iter() {
        for (i, &p) in img.pixels().iter().enumerate() {
            sum[i] += p as f64;
        }
    }
    let count = aligned_images.len() as f64;
    let stacked: Vec<f32> = sum.iter().map(|s| (*s / count) as f32).collect();

    AstroImage::from_pixels(ImageDimensions::new(width, height, channels), stacked)
}
