//! A least-squares design factored and solved at the shapes the crate fits: a cubic distortion
//! polynomial over matched stars, and a quartic background over sampled tiles.

use std::hint::black_box;

use common::bench::{self, Kind, Scale};
use criterion::{BenchmarkId, Criterion, Throughput};
use nalgebra::DMatrix;

use crate::bench::fixture;
use crate::math::lstsq::Lstsq;

/// Points and polynomial degree: 500 stars of a cubic, 10 terms; 2000 tiles of a quartic, 15.
const SHAPES: [(usize, usize); 2] = [(500, 3), (2_000, 4)];

pub(crate) fn atomic(c: &mut Criterion) {
    let mut group = Kind::Atomic.group(c, "math/lstsq", Scale::Micros);
    group.throughput(Throughput::Elements(1));
    for (points, degree) in SHAPES {
        let design = design(points, degree);
        let shape = format!("{}x{}", design.nrows(), design.ncols());
        group.bench_function(BenchmarkId::new("factor", &shape), |b| {
            b.iter_batched(
                || design.clone(),
                |design| Lstsq::new(black_box(design)),
                bench::fresh_copies(design.len() * size_of::<f64>()),
            );
        });
        group.bench_function(BenchmarkId::new("solve", &shape), |b| {
            let lstsq = Lstsq::new(design.clone());
            let rhs = DMatrix::from_iterator(
                points,
                2,
                fixture::gaussian(2 * points, 12).into_iter().map(f64::from),
            );
            b.iter(|| lstsq.solve(black_box(&rhs)));
        });
    }
    group.finish();
}

/// Every monomial `xⁱ·yʲ` with `i + j ≤ degree` at `points` points in `[−1, 1]²`.
fn design(points: usize, degree: usize) -> DMatrix<f64> {
    let coordinates = fixture::gaussian(2 * points, 11);
    let terms: Vec<(i32, i32)> = (0..=degree as i32)
        .flat_map(|total| (0..=total).map(move |i| (i, total - i)))
        .collect();
    DMatrix::from_fn(points, terms.len(), |row, col| {
        let (x, y) = (
            f64::from(coordinates[2 * row].tanh()),
            f64::from(coordinates[2 * row + 1].tanh()),
        );
        let (i, j) = terms[col];
        x.powi(i) * y.powi(j)
    })
}
