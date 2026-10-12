//! One dense solve at the sizes the crate solves: a 6-parameter fit, the SIP fallback and a
//! thin-plate spline of 64 control points.

use std::hint::black_box;

use criterion::{BenchmarkId, Criterion, Throughput};

use crate::bench::fixture::{self, Kind, Scale};
use crate::math::linear_system::solve_in_place;

const SIZES: [usize; 3] = [6, 18, 67];

pub(crate) fn atomic(c: &mut Criterion) {
    let mut group = Kind::Atomic.group(c, "math/linear_system", Scale::Sample);
    group.throughput(Throughput::Elements(1));
    for n in SIZES {
        group.bench_function(BenchmarkId::new("solve_in_place", n), |b| {
            let mut matrix: Vec<f64> = fixture::gaussian(n * n, 9)
                .into_iter()
                .map(f64::from)
                .collect();
            // Diagonally dominant, so every size is well conditioned and pivots alike.
            for i in 0..n {
                matrix[i * n + i] += n as f64;
            }
            let rhs: Vec<f64> = fixture::gaussian(n, 10)
                .into_iter()
                .map(f64::from)
                .collect();
            b.iter_batched_ref(
                || (matrix.clone(), rhs.clone()),
                |(matrix, rhs)| solve_in_place(black_box(matrix), black_box(rhs), 1e-12),
                fixture::fresh_copies((n * n + n) * size_of::<f64>()),
            );
        });
    }
    group.finish();
}
