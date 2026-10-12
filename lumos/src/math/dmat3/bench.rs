//! A transform applied to a row of points, as the resampler maps each output pixel to its source.

use std::hint::black_box;

use criterion::{BenchmarkId, Criterion, Throughput};
use glam::DVec2;

use crate::bench::fixture::{self, Scale};
use crate::math::dmat3::DMat3;

const POINTS: usize = 4_096;

pub(crate) fn benches(c: &mut Criterion) {
    let mut group = fixture::group(c, "kernel/math/dmat3", Scale::Sample);
    group.throughput(Throughput::Elements(POINTS as u64));
    for (variant, matrix) in [
        (
            "affine",
            DMat3::from_array([1.001, 0.02, 3.5, -0.02, 0.999, -2.25, 0.0, 0.0, 1.0]),
        ),
        (
            "homography",
            DMat3::from_array([1.001, 0.02, 3.5, -0.02, 0.999, -2.25, 1e-6, -2e-6, 1.0]),
        ),
    ] {
        group.bench_function(
            BenchmarkId::new(format!("transform_point/{variant}"), POINTS),
            |b| {
                let points: Vec<DVec2> =
                    (0..POINTS).map(|x| DVec2::new(x as f64, 1_000.0)).collect();
                b.iter(|| {
                    black_box(&points)
                        .iter()
                        .map(|&point| matrix.transform_point(point))
                        .fold(DVec2::ZERO, |sum, mapped| sum + mapped)
                });
            },
        );
    }
    group.finish();
}
