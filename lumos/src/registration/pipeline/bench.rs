//! Benchmark module for the full registration pipeline.
//! Run with: cargo bench -p lumos --features bench --bench registration_pipeline

use std::hint::black_box;

use criterion::{BenchmarkId, Criterion, Throughput};

use super::{Registrator, register_stars, warp_to_reference};
use crate::registration::interpolation::InterpolationMethod;
use crate::registration::types::{RegistrationConfig, TransformMatrix, TransformType};

/// Register pipeline benchmarks with Criterion.
pub fn benchmarks(c: &mut Criterion) {
    benchmark_full_pipeline(c);
    benchmark_config_variations(c);
    benchmark_warp_to_reference(c);
}

/// Generate a grid of star positions.
fn generate_star_grid(
    rows: usize,
    cols: usize,
    spacing: f64,
    offset: (f64, f64),
) -> Vec<(f64, f64)> {
    let mut stars = Vec::with_capacity(rows * cols);
    for r in 0..rows {
        for c in 0..cols {
            let x = offset.0 + c as f64 * spacing;
            let y = offset.1 + r as f64 * spacing;
            stars.push((x, y));
        }
    }
    stars
}

/// Apply a transformation to star positions.
fn transform_stars(stars: &[(f64, f64)], transform: &TransformMatrix) -> Vec<(f64, f64)> {
    stars.iter().map(|&(x, y)| transform.apply(x, y)).collect()
}

/// Benchmark the full registration pipeline with different star counts.
fn benchmark_full_pipeline(c: &mut Criterion) {
    let mut group = c.benchmark_group("registration_pipeline");

    let transform = TransformMatrix::similarity(50.0, -30.0, 0.05, 1.01);

    for star_count in [25, 50, 100, 200] {
        let side = (star_count as f64).sqrt() as usize;
        let ref_stars = generate_star_grid(side, side, 100.0, (200.0, 200.0));
        let target_stars = transform_stars(&ref_stars, &transform);

        group.throughput(Throughput::Elements(star_count as u64));

        group.bench_function(BenchmarkId::new("similarity", star_count), |b| {
            b.iter(|| {
                let result = register_stars(
                    black_box(&ref_stars),
                    black_box(&target_stars),
                    TransformType::Similarity,
                );
                black_box(result)
            })
        });
    }

    // Add outliers test
    let ref_stars = generate_star_grid(10, 10, 100.0, (200.0, 200.0));
    let mut target_stars = transform_stars(&ref_stars, &transform);
    // Add 20% outliers
    for (i, star) in target_stars.iter_mut().enumerate().take(20) {
        *star = (500.0 + i as f64 * 20.0, 500.0 - i as f64 * 10.0);
    }

    group.bench_function("with_20%_outliers", |b| {
        b.iter(|| {
            let result = register_stars(
                black_box(&ref_stars),
                black_box(&target_stars),
                TransformType::Similarity,
            );
            black_box(result)
        })
    });

    group.finish();
}

/// Benchmark different configuration options.
fn benchmark_config_variations(c: &mut Criterion) {
    let mut group = c.benchmark_group("registration_config");

    let ref_stars = generate_star_grid(8, 8, 100.0, (200.0, 200.0));

    let transforms = [
        (
            "translation",
            TransformType::Translation,
            TransformMatrix::from_translation(50.0, -30.0),
        ),
        (
            "euclidean",
            TransformType::Euclidean,
            TransformMatrix::euclidean(50.0, -30.0, 0.05),
        ),
        (
            "similarity",
            TransformType::Similarity,
            TransformMatrix::similarity(50.0, -30.0, 0.05, 1.01),
        ),
        (
            "affine",
            TransformType::Affine,
            TransformMatrix::affine([1.01, 0.05, 50.0, -0.03, 0.99, -30.0]),
        ),
    ];

    for (name, transform_type, transform) in &transforms {
        let target_stars = transform_stars(&ref_stars, transform);

        group.bench_function(*name, |b| {
            b.iter(|| {
                let result = register_stars(
                    black_box(&ref_stars),
                    black_box(&target_stars),
                    *transform_type,
                );
                black_box(result)
            })
        });
    }

    // Test different RANSAC iteration counts
    let target_stars = transform_stars(
        &ref_stars,
        &TransformMatrix::similarity(50.0, -30.0, 0.05, 1.01),
    );

    for iterations in [100, 500, 1000, 2000] {
        let config = RegistrationConfig::builder()
            .with_scale()
            .ransac_iterations(iterations)
            .build();
        let registrator = Registrator::new(config);

        group.bench_function(BenchmarkId::new("ransac_iters", iterations), |b| {
            b.iter(|| {
                let result =
                    registrator.register_positions(black_box(&ref_stars), black_box(&target_stars));
                black_box(result)
            })
        });
    }

    group.finish();
}

/// Benchmark warp_to_reference with different image sizes.
fn benchmark_warp_to_reference(c: &mut Criterion) {
    let mut group = c.benchmark_group("warp_to_reference");

    let transform = TransformMatrix::similarity(10.0, -5.0, 0.02, 1.005);

    for size in [256, 512, 1024] {
        // Generate a test image
        let image: Vec<f32> = (0..size * size)
            .map(|i| {
                let x = (i % size) as f32 / size as f32;
                let y = (i / size) as f32 / size as f32;
                (x * y).sin()
            })
            .collect();

        group.throughput(Throughput::Elements((size * size) as u64));

        group.bench_function(
            BenchmarkId::new("bilinear", format!("{}x{}", size, size)),
            |b| {
                b.iter(|| {
                    let result = warp_to_reference(
                        black_box(&image),
                        size,
                        size,
                        black_box(&transform),
                        InterpolationMethod::Bilinear,
                    );
                    black_box(result)
                })
            },
        );

        group.bench_function(
            BenchmarkId::new("lanczos3", format!("{}x{}", size, size)),
            |b| {
                b.iter(|| {
                    let result = warp_to_reference(
                        black_box(&image),
                        size,
                        size,
                        black_box(&transform),
                        InterpolationMethod::Lanczos3,
                    );
                    black_box(result)
                })
            },
        );
    }

    group.finish();
}
