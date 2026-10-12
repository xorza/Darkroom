//! The threshold over one row of a detection plane against its noise, packed into mask words, on
//! the scalar reference, every tier's kernel and the dispatcher: production thresholds the rows on
//! the global pool, a row a task.

use std::cell::OnceCell;
use std::hint::black_box;

use criterion::{BenchmarkId, Criterion, Throughput};

use crate::bench::fixture::{self, Scale};
use crate::bench::scene::{FRAME, Field};
use crate::simd::tier::Tier;
use crate::star_detection::bench::Stages;
use crate::star_detection::threshold_mask::ThresholdParams;
use crate::star_detection::threshold_mask::simd::internals::process_words_scalar;
use crate::star_detection::threshold_mask::simd::{ProcessWords, WORD_PIXELS, process_words};

pub(crate) fn benches(c: &mut Criterion) {
    let stages = OnceCell::new();
    let row = || {
        let stages: &Stages = stages.get_or_init(|| Stages::of(Field::Typical));
        let span = FRAME.height / 2 * FRAME.width..(FRAME.height / 2 + 1) * FRAME.width;
        let threshold = ThresholdParams {
            sigma: stages.config.extraction.sigma_threshold,
            min_noise: stages.plane.noise.floor,
        };
        Row {
            pixels: &stages.plane.values.pixels()[span.clone()],
            noise: &stages.plane.noise.noise.pixels()[span],
            threshold,
        }
    };
    let tiers: Vec<Tier> = Tier::supported().collect();
    let mut group = fixture::group(c, "kernel/detection/threshold_mask", Scale::Sample);
    group.throughput(Throughput::Elements(FRAME.width as u64));
    let mut words = vec![0; FRAME.width.div_ceil(WORD_PIXELS)];
    group.bench_function(BenchmarkId::new("row/scalar", FRAME.width), |b| {
        let Row {
            pixels,
            noise,
            threshold,
        } = row();
        b.iter(|| process_words_scalar(black_box(pixels), noise, threshold, &mut words));
    });
    for &tier in &tiers {
        group.bench_function(BenchmarkId::new(format!("row/{tier}"), FRAME.width), |b| {
            let Row {
                pixels,
                noise,
                threshold,
            } = row();
            b.iter(|| {
                tier.run(ProcessWords {
                    pixels: black_box(pixels),
                    noise,
                    threshold,
                    words: &mut words,
                });
            });
        });
    }
    group.bench_function(BenchmarkId::new("row/dispatch", FRAME.width), |b| {
        let Row {
            pixels,
            noise,
            threshold,
        } = row();
        b.iter(|| process_words(black_box(pixels), noise, threshold, &mut words));
    });
}

/// One row of the plane, its noise, and the threshold the detector puts on it.
#[derive(Debug)]
struct Row<'a> {
    pixels: &'a [f32],
    noise: &'a [f32],
    threshold: ThresholdParams,
}
