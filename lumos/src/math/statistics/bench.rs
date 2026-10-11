//! The order statistics over the sizes they run at: a combine pixel's frames, a star's stamp and a
//! background tile; and a whole plane's median, by radix and by selection.

use std::hint::black_box;

use criterion::{BenchmarkId, Criterion, Throughput};

use crate::bench::fixture::{self, Scale};
use crate::math::statistics::clipped_stats::ClippedStats;
use crate::math::statistics::median_mad::MedianMad;
use crate::math::statistics::radix_median::RadixMedian;
use crate::math::statistics::spread::Spread;
use crate::math::statistics::{mad_with_scratch, median_mut, robust_sigma_f64};

/// A combine pixel's frames, a star's stamp, a 64² background tile.
const LENGTHS: [usize; 3] = [16, 256, 4_096];
/// A 24 MP plane, the size normalization ranks per frame.
const PLANE: usize = 24_000_000;

pub(crate) fn benches(c: &mut Criterion) {
    let mut group = fixture::group(c, "math/statistics", Scale::Sample);
    for len in LENGTHS {
        group.throughput(Throughput::Elements(len as u64));
        group.bench_function(BenchmarkId::new("median_mut", len), |b| {
            let values = fixture::sky(len, 2);
            b.iter_batched_ref(
                || values.clone(),
                |values| median_mut(black_box(values)),
                fixture::in_place(len * size_of::<f32>()),
            );
        });
        group.bench_function(BenchmarkId::new("median_mad", len), |b| {
            let values = fixture::sky(len, 2);
            b.iter_batched_ref(
                || values.clone(),
                |values| MedianMad::of_mut(black_box(values)),
                fixture::in_place(len * size_of::<f32>()),
            );
        });
        group.bench_function(BenchmarkId::new("mad_with_scratch", len), |b| {
            let values = fixture::sky(len, 2);
            let median = median_mut(&mut values.clone());
            let mut scratch = Vec::with_capacity(len);
            b.iter(|| mad_with_scratch(black_box(&values), median, &mut scratch));
        });
        group.bench_function(BenchmarkId::new("robust_sigma_f64", len), |b| {
            let values: Vec<f64> = fixture::sky(len, 2).into_iter().map(f64::from).collect();
            let mut scratch = Vec::with_capacity(len);
            b.iter(|| robust_sigma_f64(black_box(&values), &mut scratch));
        });
        group.bench_function(BenchmarkId::new("sigma_clipped", len), |b| {
            let values = fixture::sky(len, 2);
            let mut deviations = Vec::with_capacity(len);
            b.iter_batched_ref(
                || values.clone(),
                |values| ClippedStats::sigma_clipped(black_box(values), &mut deviations, 3.0, 5),
                fixture::in_place(len * size_of::<f32>()),
            );
        });
        group.bench_function(BenchmarkId::new("spread", len), |b| {
            let mut sorted = fixture::sky(len, 2);
            sorted.sort_unstable_by(f32::total_cmp);
            b.iter(|| Spread::of_sorted(black_box(&sorted)));
        });
    }
    group.finish();

    let mut group = fixture::group(c, "math/statistics/frame", Scale::Frame);
    group.throughput(Throughput::Elements(PLANE as u64));
    group.bench_function(BenchmarkId::new("plane_median/radix", PLANE), |b| {
        let plane = fixture::sky(PLANE, 3);
        let mut median = RadixMedian::default();
        b.iter(|| {
            let mut high = median.high_pass();
            for &value in black_box(&plane) {
                high.add(value);
            }
            let mut low = high.finish();
            for &value in &plane {
                low.add(value);
            }
            low.median()
        });
    });
    // The plane must keep its order, so selecting its median takes a copy of it: the copy is part
    // of what this way costs.
    group.bench_function(BenchmarkId::new("plane_median/selection", PLANE), |b| {
        let plane = fixture::sky(PLANE, 3);
        let mut copy = Vec::with_capacity(PLANE);
        b.iter(|| {
            copy.clear();
            copy.extend_from_slice(black_box(&plane));
            median_mut(&mut copy)
        });
    });
    group.finish();
}
