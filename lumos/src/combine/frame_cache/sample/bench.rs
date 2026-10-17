//! The quality the standard planes read of one block of pixels, a pixel at a time, as the combine
//! measures it of every pixel's survivors: the summed weight, the inverse variance by each
//! sample's noise model, and the dispersion. All 30 samples of each pixel survive.

use std::hint::black_box;

use criterion::{BenchmarkId, Criterion, Throughput};

use crate::bench::fixture::{Kind, Scale};
use crate::bench::scene::STACKED_LIGHTS;
use crate::combine::reducer::combined_sample::CombinedSample;
use crate::combine::reducer::pixel_samples::PixelSamples;
use crate::combine::rejection::bench::{BLOCK, Block};
use crate::math::sum;

pub(crate) fn atomic(c: &mut Criterion) {
    let frames = STACKED_LIGHTS;
    let mut group = Kind::Atomic.group(c, "combine/sample", Scale::Sample);
    group.throughput(Throughput::Elements((BLOCK.pixel_count() * frames) as u64));
    group.bench_function(BenchmarkId::new("from_survivors", frames), |b| {
        let block = Block::of(frames);
        let means: Vec<f32> = block
            .samples
            .chunks_exact(frames)
            .map(|pixel| sum::weighted_mean_f32(pixel, &block.weights))
            .collect();
        let mut samples = block.samples.clone();
        b.iter(|| {
            for (values, &mean) in samples.chunks_exact_mut(frames).zip(&means) {
                let pixel = PixelSamples {
                    values,
                    weights: &block.weights,
                    noise: Some(block.noise()),
                };
                black_box(CombinedSample::from_survivors(mean, &pixel, 0..frames));
            }
        });
    });
}
