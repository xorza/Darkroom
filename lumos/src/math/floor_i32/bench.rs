//! [`FloorI32`] against the library `floor`, at both precisions.

use std::hint::black_box;

use common::bench::{Kind, Scale};
use criterion::{BenchmarkId, Criterion, Throughput};

use crate::bench::fixture;
use crate::math::floor_i32::FloorI32;

const VALUES: usize = 4_096;

pub(crate) fn atomic(c: &mut Criterion) {
    let mut group = Kind::Atomic.group(c, "math/floor_i32", Scale::Micros);
    group.throughput(Throughput::Elements(VALUES as u64));
    group.bench_function(BenchmarkId::new("floor_i32/f32", VALUES), |b| {
        let values = values();
        b.iter(|| {
            black_box(&values)
                .iter()
                .map(|&v| v.floor_i32())
                .sum::<i32>()
        });
    });
    group.bench_function(BenchmarkId::new("floor_i32/f64", VALUES), |b| {
        let values: Vec<f64> = values().into_iter().map(f64::from).collect();
        b.iter(|| {
            black_box(&values)
                .iter()
                .map(|&v| v.floor_i32())
                .sum::<i32>()
        });
    });
    group.bench_function(BenchmarkId::new("floor_std/f32", VALUES), |b| {
        let values = values();
        b.iter(|| {
            black_box(&values)
                .iter()
                .map(|&v| v.floor() as i32)
                .sum::<i32>()
        });
    });
    group.bench_function(BenchmarkId::new("floor_std/f64", VALUES), |b| {
        let values: Vec<f64> = values().into_iter().map(f64::from).collect();
        b.iter(|| {
            black_box(&values)
                .iter()
                .map(|&v| v.floor() as i32)
                .sum::<i32>()
        });
    });
    group.finish();
}

/// Source coordinates on both sides of 0, as a warp near the frame's edge reads them.
fn values() -> Vec<f32> {
    fixture::gaussian(VALUES, 8)
        .into_iter()
        .map(|v| v * 500.0)
        .collect()
}
