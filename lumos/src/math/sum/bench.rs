//! The sums over the length range the vector gates sit in, on the scalar loop, every tier's
//! kernel and the dispatcher. Every tier is measured below its gate too, which is the measurement
//! that sets the gate: under it, the dispatcher is the scalar path.

use std::hint::black_box;

use criterion::{BenchmarkId, Criterion, Throughput};

use crate::bench::fixture::{self, Kind, Scale};
use crate::math::sum::simd::{SumF32, WeightedSumsKernel};
use crate::math::sum::{scalar, sum_f32, weighted_sums};
use crate::simd::isa_tier::IsaTier;

/// The 6/10/12/24 rungs measure the range about the one-vector gate: steps of 4, 8, 16 jump clean
/// over it.
const LENGTHS: [usize; 13] = [1, 4, 6, 8, 10, 12, 16, 24, 32, 64, 256, 1_024, 10_000];

pub(crate) fn atomic(c: &mut Criterion) {
    let tiers: Vec<IsaTier> = IsaTier::supported().collect();
    let mut group = Kind::Atomic.group(c, "math/sum", Scale::Sample);
    for len in LENGTHS {
        group.throughput(Throughput::Elements(len as u64));
        group.bench_function(BenchmarkId::new("sum_f32/scalar", len), |b| {
            let values = fixture::gaussian(len, 1);
            b.iter(|| scalar::sum_f32(black_box(&values)));
        });
        for &tier in &tiers {
            group.bench_function(BenchmarkId::new(format!("sum_f32/{tier}"), len), |b| {
                let values = fixture::gaussian(len, 1);
                b.iter(|| tier.run(SumF32(black_box(&values))));
            });
        }
        group.bench_function(BenchmarkId::new("sum_f32/dispatch", len), |b| {
            let values = fixture::gaussian(len, 1);
            b.iter(|| sum_f32(black_box(&values)));
        });

        group.bench_function(BenchmarkId::new("weighted_sums/scalar", len), |b| {
            let (values, weights) = (fixture::gaussian(len, 1), weights(len));
            b.iter(|| scalar::weighted_sums(black_box(&values), black_box(&weights)));
        });
        for &tier in &tiers {
            group.bench_function(
                BenchmarkId::new(format!("weighted_sums/{}", tier.id()), len),
                |b| {
                    let (values, weights) = (fixture::gaussian(len, 1), weights(len));
                    b.iter(|| {
                        tier.run(WeightedSumsKernel {
                            values: black_box(&values),
                            weights: black_box(&weights),
                        })
                    });
                },
            );
        }
        group.bench_function(BenchmarkId::new("weighted_sums/dispatch", len), |b| {
            let (values, weights) = (fixture::gaussian(len, 1), weights(len));
            b.iter(|| weighted_sums(black_box(&values), black_box(&weights)));
        });
    }
    group.finish();
}

/// Frame trust factors: positive, and not all equal.
fn weights(len: usize) -> Vec<f32> {
    (0..len).map(|i| 1.0 + i as f32 * 0.001).collect()
}
