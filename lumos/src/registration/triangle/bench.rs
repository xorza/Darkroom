//! Benchmark module for triangle matching.
//! Run with: cargo bench -p lumos --features bench --bench registration_triangle

use std::hint::black_box;
use std::path::Path;

use criterion::{BenchmarkId, Criterion, Throughput};

use super::{
    Triangle, TriangleHashTable, TriangleMatchConfig, form_triangles, match_stars_triangles,
};

/// Register triangle matching benchmarks with Criterion.
pub fn benchmarks(c: &mut Criterion, _calibration_dir: &Path) {
    benchmark_triangle_formation(c);
    benchmark_hash_table_build(c);
    benchmark_hash_table_lookup(c);
    benchmark_match_stars(c);
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
fn transform_stars(stars: &[(f64, f64)], dx: f64, dy: f64, angle: f64) -> Vec<(f64, f64)> {
    let cos_a = angle.cos();
    let sin_a = angle.sin();
    stars
        .iter()
        .map(|&(x, y)| {
            let rx = x * cos_a - y * sin_a + dx;
            let ry = x * sin_a + y * cos_a + dy;
            (rx, ry)
        })
        .collect()
}

/// Benchmark triangle formation from star positions.
fn benchmark_triangle_formation(c: &mut Criterion) {
    let mut group = c.benchmark_group("triangle_formation");

    for star_count in [20, 50, 100, 200] {
        let stars = generate_star_grid(
            (star_count as f64).sqrt() as usize,
            (star_count as f64).sqrt() as usize,
            100.0,
            (100.0, 100.0),
        );

        group.throughput(Throughput::Elements(star_count as u64));

        group.bench_function(BenchmarkId::new("form", star_count), |b| {
            b.iter(|| {
                let triangles = form_triangles(black_box(&stars), black_box(star_count));
                black_box(triangles)
            })
        });
    }

    group.finish();
}

/// Benchmark hash table construction.
fn benchmark_hash_table_build(c: &mut Criterion) {
    let mut group = c.benchmark_group("triangle_hash_table_build");

    for star_count in [20, 50, 100] {
        let side = (star_count as f64).sqrt() as usize;
        let stars = generate_star_grid(side, side, 100.0, (100.0, 100.0));
        let triangles = form_triangles(&stars, star_count);

        group.throughput(Throughput::Elements(triangles.len() as u64));

        group.bench_function(BenchmarkId::new("build", star_count), |b| {
            b.iter(|| {
                let table = TriangleHashTable::build(black_box(&triangles), 100);
                black_box(table)
            })
        });
    }

    group.finish();
}

/// Benchmark hash table lookup.
fn benchmark_hash_table_lookup(c: &mut Criterion) {
    let mut group = c.benchmark_group("triangle_hash_table_lookup");

    let stars = generate_star_grid(10, 10, 100.0, (100.0, 100.0));
    let triangles = form_triangles(&stars, 100);
    let table = TriangleHashTable::build(&triangles, 100);

    // Create a query triangle from the first three stars
    let query = Triangle::from_positions(
        [0, 1, 2],
        [
            (stars[0].0, stars[0].1),
            (stars[1].0, stars[1].1),
            (stars[2].0, stars[2].1),
        ],
    )
    .expect("Valid triangle");

    group.bench_function("single_lookup", |b| {
        b.iter(|| {
            let candidates = table.find_candidates(black_box(&query), black_box(0.01));
            black_box(candidates)
        })
    });

    group.finish();
}

/// Benchmark full star matching pipeline.
fn benchmark_match_stars(c: &mut Criterion) {
    let mut group = c.benchmark_group("match_stars");

    let config = TriangleMatchConfig {
        max_stars: 100,
        ratio_tolerance: 0.01,
        min_votes: 2,
        hash_bins: 100,
        check_orientation: true,
    };

    for star_count in [25, 50, 100] {
        let side = (star_count as f64).sqrt() as usize;
        let ref_stars = generate_star_grid(side, side, 100.0, (100.0, 100.0));

        // Create target stars with translation and rotation
        let target_stars = transform_stars(&ref_stars, 50.0, -30.0, 0.05);

        group.throughput(Throughput::Elements(star_count as u64));

        group.bench_function(BenchmarkId::new("full_match", star_count), |b| {
            b.iter(|| {
                let matches = match_stars_triangles(
                    black_box(&ref_stars),
                    black_box(&target_stars),
                    black_box(&config),
                );
                black_box(matches)
            })
        });
    }

    // Test with outliers
    let ref_stars = generate_star_grid(7, 7, 100.0, (100.0, 100.0));
    let mut target_stars = transform_stars(&ref_stars, 25.0, 40.0, 0.02);
    // Add some outliers
    target_stars[0] = (500.0, 500.0);
    target_stars[10] = (50.0, 800.0);

    group.bench_function("with_outliers", |b| {
        b.iter(|| {
            let matches = match_stars_triangles(
                black_box(&ref_stars),
                black_box(&target_stars),
                black_box(&config),
            );
            black_box(matches)
        })
    });

    group.finish();
}
