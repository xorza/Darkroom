//! Benchmarks for 3x3 median filter.
//! Run with: cargo bench -p lumos --features bench --bench median_filter

use std::hint::black_box;

use criterion::{BenchmarkId, Criterion, Throughput, criterion_group};

use super::median_filter_3x3;
use super::simd::{median_filter_row_scalar, median_filter_row_simd};

#[allow(dead_code)]
pub fn bench_median_filter(c: &mut Criterion) {
    let mut group = c.benchmark_group("median_filter_3x3");

    for (width, height) in [(512, 512), (1024, 1024), (4096, 4096)] {
        let pixels: Vec<f32> = (0..width * height)
            .map(|i| (i % 256) as f32 / 255.0)
            .collect();

        group.throughput(Throughput::Elements((width * height) as u64));
        group.bench_with_input(
            BenchmarkId::new("size", format!("{}x{}", width, height)),
            &pixels,
            |b, pixels| {
                b.iter(|| {
                    black_box(median_filter_3x3(black_box(pixels), width, height));
                });
            },
        );
    }

    group.finish();

    // SIMD vs Scalar comparison for row processing
    let mut simd_group = c.benchmark_group("median_filter_row");

    for width in [256, 512, 1024, 2048] {
        let row_above: Vec<f32> = (0..width).map(|i| ((i * 3) % 100) as f32 * 0.01).collect();
        let row_curr: Vec<f32> = (0..width).map(|i| ((i * 7) % 100) as f32 * 0.01).collect();
        let row_below: Vec<f32> = (0..width).map(|i| ((i * 11) % 100) as f32 * 0.01).collect();
        let mut output_scalar = vec![0.0f32; width];
        let mut output_simd = vec![0.0f32; width];

        simd_group.throughput(Throughput::Elements(width as u64));

        simd_group.bench_function(BenchmarkId::new("scalar", width), |b| {
            b.iter(|| {
                median_filter_row_scalar(
                    black_box(&row_above),
                    black_box(&row_curr),
                    black_box(&row_below),
                    black_box(&mut output_scalar),
                    width,
                );
            });
        });

        simd_group.bench_function(BenchmarkId::new("simd", width), |b| {
            b.iter(|| {
                median_filter_row_simd(
                    black_box(&row_above),
                    black_box(&row_curr),
                    black_box(&row_below),
                    black_box(&mut output_simd),
                    width,
                );
            });
        });
    }

    simd_group.finish();
}

criterion_group!(benches, bench_median_filter);
