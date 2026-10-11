//! The noise estimators on whole frames, and the gain binning they share: a frame's samples
//! binned one by one, then grouped and measured bin by bin.

use std::hint::black_box;

use criterion::{BatchSize, BenchmarkId, Criterion, Throughput};

use crate::bench::fixture::{self, Scale};
use crate::math::noise::binned_samples::BinnedSamples;
use crate::math::noise::gain_bins::GainBins;
use crate::math::noise::mrs_noise::MrsNoise;
use crate::math::size2us::Size2us;

/// One tile of the MRS grid, and a frame the 4 × 4 grid covers in part.
const SIDES: [usize; 2] = [1_024, 4_096];
/// The samples one gain-binned estimate measures at most.
const SAMPLES: usize = 1_000_000;
const GAINS: usize = 4_096;

pub(crate) fn benches(c: &mut Criterion) {
    let mut group = fixture::group(c, "math/noise", Scale::Sample);
    group.throughput(Throughput::Elements(GAINS as u64));
    group.bench_function(BenchmarkId::new("gain_bin", GAINS), |b| {
        let bins = GainBins::of(&mut vignette(GAINS));
        let gains = vignette(GAINS);
        b.iter(|| {
            black_box(&gains)
                .iter()
                .map(|&gain| bins.bin(gain))
                .sum::<usize>()
        });
    });
    group.finish();

    let mut group = fixture::group(c, "math/noise/frame", Scale::Frame);
    group.throughput(Throughput::Elements(SAMPLES as u64));
    group.bench_function(BenchmarkId::new("binned_measure", SAMPLES), |b| {
        let samples = fixture::sky(SAMPLES, 4);
        let gains = vignette(SAMPLES);
        let bins = GainBins::of(&mut gains.clone());
        b.iter_batched(
            || {
                let mut binned = BinnedSamples::with_capacity(SAMPLES);
                for (&sample, &gain) in samples.iter().zip(&gains) {
                    binned.push(&bins, gain, sample);
                }
                binned
            },
            |binned| binned.measure(|run| run.iter().map(|&v| f64::from(v * v)).sum::<f64>()),
            BatchSize::LargeInput,
        );
    });
    for side in SIDES {
        let size = Size2us::new(side, side);
        group.throughput(Throughput::Elements(size.pixel_count() as u64));
        group.bench_function(BenchmarkId::new("mrs_estimate", side), |b| {
            let plane = fixture::sky(size.pixel_count(), 5);
            b.iter(|| MrsNoise::estimate(black_box(&plane), size, |_| false));
        });
        group.bench_function(BenchmarkId::new("mrs_estimate_split", side), |b| {
            let plane = fixture::sky(size.pixel_count(), 5);
            let gains = vignette(size.pixel_count());
            let bins = GainBins::of(&mut gains.clone());
            b.iter(|| {
                MrsNoise::estimate_split(
                    black_box(&plane),
                    size,
                    |_| false,
                    |index| gains[index],
                    &bins,
                )
            });
        });
    }
    group.finish();
}

/// `len` flat gains rising from 1 at the first sample to 1.3 at the last, as a vignette spreads
/// them.
fn vignette(len: usize) -> Vec<f32> {
    (0..len)
        .map(|i| 1.0 + 0.3 * i as f32 / len as f32)
        .collect()
}
