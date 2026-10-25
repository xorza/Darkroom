//! One dense factorization and solve at 6, 18 and 67 unknowns: a thin-plate spline of 64 control
//! points is the last.

use std::hint::black_box;

use common::bench::{self, Kind, Scale};
use criterion::{BenchmarkId, Criterion, Throughput};

use crate::bench::fixture;
use crate::math::linear_system::Lu;

const SIZES: [usize; 3] = [6, 18, 67];

pub(crate) fn atomic(c: &mut Criterion) {
    let mut group = Kind::Atomic.group(c, "math/linear_system", Scale::Micros);
    group.throughput(Throughput::Elements(1));
    for n in SIZES {
        group.bench_function(BenchmarkId::new("factor_and_solve", n), |b| {
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
                || (matrix.clone(), rhs.clone(), vec![0; n]),
                |(matrix, rhs, pivots)| {
                    if let Some(lu) = Lu::factor(black_box(matrix), pivots, 1e-12) {
                        lu.solve(black_box(rhs));
                    }
                },
                bench::fresh_copies((n * n + n) * size_of::<f64>()),
            );
        });
    }
    group.finish();
}
