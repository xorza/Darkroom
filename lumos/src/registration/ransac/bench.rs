//! Benchmark module for RANSAC estimation.
//! Run with: cargo bench -p lumos --features bench --bench registration_ransac

use std::hint::black_box;
use std::path::Path;

use criterion::{BenchmarkId, Criterion, Throughput};

use super::simd::{compute_residuals_simd, count_inliers_simd};
use super::{RansacConfig, RansacEstimator};
use crate::registration::types::{TransformMatrix, TransformType};

/// Register RANSAC benchmarks with Criterion.
pub fn benchmarks(c: &mut Criterion, _calibration_dir: &Path) {
    benchmark_ransac_estimation(c);
    benchmark_transform_types(c);
    benchmark_outlier_ratios(c);
    benchmark_refinement(c);
    benchmark_simd_vs_scalar(c);
}

/// Generate matched point pairs with optional outliers.
fn generate_point_pairs(
    count: usize,
    transform: &TransformMatrix,
    outlier_ratio: f64,
) -> (Vec<(f64, f64)>, Vec<(f64, f64)>) {
    let mut ref_points = Vec::with_capacity(count);
    let mut target_points = Vec::with_capacity(count);

    let outlier_count = (count as f64 * outlier_ratio) as usize;

    for i in 0..count {
        let x = 100.0 + (i % 20) as f64 * 50.0;
        let y = 100.0 + (i / 20) as f64 * 50.0;
        ref_points.push((x, y));

        if i < outlier_count {
            // Outlier - random position
            target_points.push((500.0 + i as f64 * 10.0, 500.0 - i as f64 * 5.0));
        } else {
            // Inlier - transformed position
            target_points.push(transform.apply(x, y));
        }
    }

    (ref_points, target_points)
}

/// Benchmark RANSAC estimation with different point counts.
fn benchmark_ransac_estimation(c: &mut Criterion) {
    let mut group = c.benchmark_group("ransac_estimation");

    let config = RansacConfig {
        max_iterations: 1000,
        inlier_threshold: 2.0,
        confidence: 0.99,
        min_inlier_ratio: 0.3,
        seed: Some(42),
        use_local_optimization: true,
        lo_max_iterations: 10,
    };
    let estimator = RansacEstimator::new(config);

    let transform = TransformMatrix::similarity(50.0, -30.0, 0.1, 1.02);

    for point_count in [20, 50, 100, 200] {
        let (ref_points, target_points) = generate_point_pairs(point_count, &transform, 0.2);

        group.throughput(Throughput::Elements(point_count as u64));

        group.bench_function(BenchmarkId::new("similarity", point_count), |b| {
            b.iter(|| {
                let result = estimator.estimate(
                    black_box(&ref_points),
                    black_box(&target_points),
                    TransformType::Similarity,
                );
                black_box(result)
            })
        });
    }

    group.finish();
}

/// Benchmark different transform types.
fn benchmark_transform_types(c: &mut Criterion) {
    let mut group = c.benchmark_group("ransac_transform_types");

    let config = RansacConfig {
        max_iterations: 1000,
        inlier_threshold: 2.0,
        confidence: 0.99,
        min_inlier_ratio: 0.3,
        seed: Some(42),
        use_local_optimization: true,
        lo_max_iterations: 10,
    };
    let estimator = RansacEstimator::new(config);

    let transforms = [
        (
            TransformType::Translation,
            TransformMatrix::from_translation(50.0, -30.0),
        ),
        (
            TransformType::Euclidean,
            TransformMatrix::euclidean(50.0, -30.0, 0.1),
        ),
        (
            TransformType::Similarity,
            TransformMatrix::similarity(50.0, -30.0, 0.1, 1.02),
        ),
        (
            TransformType::Affine,
            TransformMatrix::affine([1.02, 0.1, 50.0, -0.05, 0.98, -30.0]),
        ),
    ];

    for (transform_type, transform) in &transforms {
        let (ref_points, target_points) = generate_point_pairs(100, transform, 0.2);
        let type_name = format!("{:?}", transform_type);

        group.bench_function(&type_name, |b| {
            b.iter(|| {
                let result = estimator.estimate(
                    black_box(&ref_points),
                    black_box(&target_points),
                    *transform_type,
                );
                black_box(result)
            })
        });
    }

    group.finish();
}

/// Benchmark with different outlier ratios.
fn benchmark_outlier_ratios(c: &mut Criterion) {
    let mut group = c.benchmark_group("ransac_outlier_ratios");

    let config = RansacConfig {
        max_iterations: 2000,
        inlier_threshold: 2.0,
        confidence: 0.99,
        min_inlier_ratio: 0.3,
        seed: Some(42),
        use_local_optimization: true,
        lo_max_iterations: 10,
    };
    let estimator = RansacEstimator::new(config);

    let transform = TransformMatrix::similarity(50.0, -30.0, 0.1, 1.02);

    for outlier_percent in [10, 20, 30, 40] {
        let outlier_ratio = outlier_percent as f64 / 100.0;
        let (ref_points, target_points) = generate_point_pairs(100, &transform, outlier_ratio);

        group.bench_function(
            BenchmarkId::new("outliers", format!("{}%", outlier_percent)),
            |b| {
                b.iter(|| {
                    let result = estimator.estimate(
                        black_box(&ref_points),
                        black_box(&target_points),
                        TransformType::Similarity,
                    );
                    black_box(result)
                })
            },
        );
    }

    group.finish();
}

/// Benchmark least squares refinement.
fn benchmark_refinement(c: &mut Criterion) {
    let mut group = c.benchmark_group("ransac_refinement");

    let transform = TransformMatrix::similarity(50.0, -30.0, 0.1, 1.02);

    for point_count in [10, 20, 50, 100] {
        let ref_points: Vec<(f64, f64)> = (0..point_count)
            .map(|i| {
                (
                    100.0 + (i % 10) as f64 * 50.0,
                    100.0 + (i / 10) as f64 * 50.0,
                )
            })
            .collect();
        let target_points: Vec<(f64, f64)> = ref_points
            .iter()
            .map(|&(x, y)| transform.apply(x, y))
            .collect();

        group.throughput(Throughput::Elements(point_count as u64));

        group.bench_function(BenchmarkId::new("least_squares", point_count), |b| {
            b.iter(|| {
                let result =
                    super::estimate_similarity(black_box(&ref_points), black_box(&target_points));
                black_box(result)
            })
        });
    }

    group.finish();
}

/// Benchmark SIMD vs scalar implementations for inlier counting and residuals.
fn benchmark_simd_vs_scalar(c: &mut Criterion) {
    let mut group = c.benchmark_group("ransac_simd_vs_scalar");

    let transform = TransformMatrix::similarity(50.0, -30.0, 0.1, 1.02);
    let threshold = 2.0;

    for point_count in [50, 100, 200, 500] {
        // Generate points with ~20% outliers
        let ref_points: Vec<(f64, f64)> = (0..point_count)
            .map(|i| {
                (
                    100.0 + (i % 20) as f64 * 50.0,
                    100.0 + (i / 20) as f64 * 50.0,
                )
            })
            .collect();

        let target_points: Vec<(f64, f64)> = ref_points
            .iter()
            .enumerate()
            .map(|(i, &(x, y))| {
                let (tx, ty) = transform.apply(x, y);
                if i % 5 == 0 {
                    (tx + 100.0, ty + 100.0) // outlier
                } else {
                    (tx + 0.1, ty - 0.1) // small noise
                }
            })
            .collect();

        group.throughput(Throughput::Elements(point_count as u64));

        // Count inliers benchmark (SIMD dispatch)
        group.bench_function(BenchmarkId::new("count_inliers_simd", point_count), |b| {
            b.iter(|| {
                let result = count_inliers_simd(
                    black_box(&ref_points),
                    black_box(&target_points),
                    black_box(&transform),
                    threshold,
                );
                black_box(result)
            })
        });

        // Compute residuals benchmark (SIMD dispatch)
        group.bench_function(
            BenchmarkId::new("compute_residuals_simd", point_count),
            |b| {
                b.iter(|| {
                    let result = compute_residuals_simd(
                        black_box(&ref_points),
                        black_box(&target_points),
                        black_box(&transform),
                    );
                    black_box(result)
                })
            },
        );
    }

    group.finish();
}
