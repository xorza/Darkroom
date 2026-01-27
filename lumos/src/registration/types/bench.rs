//! Benchmark module for registration types.
//! Run with: cargo bench -p lumos --features bench --bench registration_types

use std::hint::black_box;

use criterion::{BenchmarkId, Criterion, Throughput};

use super::TransformMatrix;

/// Register types benchmarks with Criterion.
pub fn benchmarks(c: &mut Criterion) {
    benchmark_apply(c);
    benchmark_matrix_inverse(c);
    benchmark_matrix_compose(c);
    benchmark_transform_batch(c);
}

/// Benchmark transforming a single point with different transform types.
fn benchmark_apply(c: &mut Criterion) {
    let mut group = c.benchmark_group("apply");

    let transforms = [
        ("translation", TransformMatrix::translation(10.0, 20.0)),
        ("euclidean", TransformMatrix::euclidean(10.0, 20.0, 0.1)),
        (
            "similarity",
            TransformMatrix::similarity(10.0, 20.0, 0.1, 1.05),
        ),
        (
            "affine",
            TransformMatrix::affine([1.0, 0.1, 10.0, -0.1, 1.0, 20.0]),
        ),
        (
            "homography",
            TransformMatrix::homography([1.0, 0.1, 10.0, -0.1, 1.0, 20.0, 0.0001, 0.0001]),
        ),
    ];

    for (name, transform) in &transforms {
        group.bench_function(BenchmarkId::new("single", *name), |b| {
            b.iter(|| {
                let result = transform.apply(black_box(100.0), black_box(200.0));
                black_box(result)
            })
        });
    }

    group.finish();
}

/// Benchmark matrix inverse computation.
fn benchmark_matrix_inverse(c: &mut Criterion) {
    let mut group = c.benchmark_group("matrix_inverse");

    let transforms = [
        ("translation", TransformMatrix::translation(10.0, 20.0)),
        ("euclidean", TransformMatrix::euclidean(10.0, 20.0, 0.1)),
        (
            "similarity",
            TransformMatrix::similarity(10.0, 20.0, 0.1, 1.05),
        ),
        (
            "affine",
            TransformMatrix::affine([1.0, 0.1, 10.0, -0.1, 1.0, 20.0]),
        ),
        (
            "homography",
            TransformMatrix::homography([1.0, 0.1, 10.0, -0.1, 1.0, 20.0, 0.0001, 0.0001]),
        ),
    ];

    for (name, transform) in &transforms {
        group.bench_function(*name, |b| {
            b.iter(|| {
                let result = black_box(transform).inverse();
                black_box(result)
            })
        });
    }

    group.finish();
}

/// Benchmark matrix composition.
fn benchmark_matrix_compose(c: &mut Criterion) {
    let mut group = c.benchmark_group("matrix_compose");

    let t1 = TransformMatrix::translation(10.0, 20.0);
    let t2 = TransformMatrix::euclidean(5.0, -5.0, 0.05);
    let t3 = TransformMatrix::similarity(0.0, 0.0, 0.1, 1.02);

    group.bench_function("two_transforms", |b| {
        b.iter(|| {
            let result = black_box(&t1).compose(black_box(&t2));
            black_box(result)
        })
    });

    group.bench_function("three_transforms", |b| {
        b.iter(|| {
            let result = black_box(&t1)
                .compose(black_box(&t2))
                .compose(black_box(&t3));
            black_box(result)
        })
    });

    group.finish();
}

/// Benchmark transforming batches of points.
fn benchmark_transform_batch(c: &mut Criterion) {
    let mut group = c.benchmark_group("transform_batch");

    let transform = TransformMatrix::similarity(10.0, 20.0, 0.1, 1.05);

    for count in [100, 1000, 10000] {
        let points: Vec<(f64, f64)> = (0..count)
            .map(|i| (i as f64 * 10.0, i as f64 * 5.0))
            .collect();

        group.throughput(Throughput::Elements(count as u64));

        group.bench_function(BenchmarkId::new("sequential", count), |b| {
            b.iter(|| {
                let results: Vec<_> = black_box(&points)
                    .iter()
                    .map(|&(x, y)| transform.apply(x, y))
                    .collect();
                black_box(results)
            })
        });
    }

    group.finish();
}
