//! Benchmark module for quality metrics.
//! Run with: cargo bench -p lumos --features bench --bench registration_quality

use std::hint::black_box;

use criterion::{BenchmarkId, Criterion, Throughput};

use super::{
    QualityMetrics, ResidualStats, check_quadrant_consistency, compute_residuals, estimate_overlap,
};
use crate::registration::types::TransformMatrix;

/// Register quality metrics benchmarks with Criterion.
pub fn benchmarks(c: &mut Criterion) {
    benchmark_quality_metrics(c);
    benchmark_residual_computation(c);
    benchmark_residual_stats(c);
    benchmark_quadrant_consistency(c);
    benchmark_overlap_estimation(c);
}

/// Generate matched point pairs for testing.
#[allow(clippy::type_complexity)]
fn generate_point_pairs(
    count: usize,
    transform: &TransformMatrix,
) -> (Vec<(f64, f64)>, Vec<(f64, f64)>) {
    let ref_points: Vec<(f64, f64)> = (0..count)
        .map(|i| {
            let x = 100.0 + (i % 20) as f64 * 50.0;
            let y = 100.0 + (i / 20) as f64 * 50.0;
            (x, y)
        })
        .collect();

    let target_points: Vec<(f64, f64)> = ref_points
        .iter()
        .map(|&(x, y)| transform.apply(x, y))
        .collect();

    (ref_points, target_points)
}

/// Benchmark quality metrics computation.
fn benchmark_quality_metrics(c: &mut Criterion) {
    let mut group = c.benchmark_group("quality_metrics");

    for residual_count in [10, 50, 100, 500] {
        // Generate residuals with some variation
        let residuals: Vec<f64> = (0..residual_count)
            .map(|i| 0.1 + (i as f64 * 0.01).sin().abs() * 0.5)
            .collect();

        group.throughput(Throughput::Elements(residual_count as u64));

        group.bench_function(BenchmarkId::new("compute", residual_count), |b| {
            b.iter(|| {
                let metrics = QualityMetrics::compute(
                    black_box(&residuals),
                    black_box(residual_count + 10), // num_matches
                    black_box(residual_count),      // num_inliers
                    black_box(0.95),                // overlap_fraction
                );
                black_box(metrics)
            })
        });
    }

    group.finish();
}

/// Benchmark residual computation.
fn benchmark_residual_computation(c: &mut Criterion) {
    let mut group = c.benchmark_group("residual_computation");

    let transform = TransformMatrix::similarity(50.0, -30.0, 0.05, 1.01);

    for point_count in [20, 50, 100, 200] {
        let (ref_points, target_points) = generate_point_pairs(point_count, &transform);

        group.throughput(Throughput::Elements(point_count as u64));

        group.bench_function(BenchmarkId::new("compute", point_count), |b| {
            b.iter(|| {
                let residuals = compute_residuals(
                    black_box(&ref_points),
                    black_box(&target_points),
                    black_box(&transform),
                );
                black_box(residuals)
            })
        });
    }

    group.finish();
}

/// Benchmark residual statistics computation.
fn benchmark_residual_stats(c: &mut Criterion) {
    let mut group = c.benchmark_group("residual_stats");

    for count in [20, 100, 500] {
        let residuals: Vec<f64> = (0..count)
            .map(|i| 0.1 + (i as f64 * 0.02).sin().abs() * 0.8)
            .collect();

        group.throughput(Throughput::Elements(count as u64));

        group.bench_function(BenchmarkId::new("compute", count), |b| {
            b.iter(|| {
                let stats = ResidualStats::compute(black_box(&residuals));
                black_box(stats)
            })
        });
    }

    group.finish();
}

/// Benchmark quadrant consistency check.
fn benchmark_quadrant_consistency(c: &mut Criterion) {
    let mut group = c.benchmark_group("quadrant_consistency");

    let transform = TransformMatrix::similarity(50.0, -30.0, 0.05, 1.01);

    for point_count in [20, 50, 100] {
        // Generate points spread across all quadrants
        let ref_points: Vec<(f64, f64)> = (0..point_count)
            .map(|i| {
                let angle = i as f64 * std::f64::consts::TAU / point_count as f64;
                let r = 300.0 + (i % 5) as f64 * 20.0;
                (500.0 + r * angle.cos(), 500.0 + r * angle.sin())
            })
            .collect();

        let target_points: Vec<(f64, f64)> = ref_points
            .iter()
            .map(|&(x, y)| transform.apply(x, y))
            .collect();

        group.throughput(Throughput::Elements(point_count as u64));

        group.bench_function(BenchmarkId::new("check", point_count), |b| {
            b.iter(|| {
                let consistency = check_quadrant_consistency(
                    black_box(&ref_points),
                    black_box(&target_points),
                    black_box(&transform),
                    1000,
                    1000,
                );
                black_box(consistency)
            })
        });
    }

    group.finish();
}

/// Benchmark overlap estimation.
fn benchmark_overlap_estimation(c: &mut Criterion) {
    let mut group = c.benchmark_group("overlap_estimation");

    let transforms = [
        ("identity", TransformMatrix::identity()),
        (
            "translation_small",
            TransformMatrix::translation(50.0, 30.0),
        ),
        (
            "translation_large",
            TransformMatrix::translation(500.0, 300.0),
        ),
        (
            "rotation",
            TransformMatrix::rotation_around(500.0, 500.0, 0.1),
        ),
        (
            "similarity",
            TransformMatrix::similarity(50.0, 30.0, 0.1, 1.1),
        ),
    ];

    for (name, transform) in &transforms {
        group.bench_function(*name, |b| {
            b.iter(|| {
                let overlap = estimate_overlap(1000, 1000, black_box(transform));
                black_box(overlap)
            })
        });
    }

    group.finish();
}
