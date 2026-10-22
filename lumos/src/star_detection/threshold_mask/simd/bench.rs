//! The threshold over one row of a detection plane against its noise, packed into mask words, on
//! the scalar reference, every tier's kernel and the dispatcher: production thresholds the rows on
//! the global pool, a row a task.

use std::cell::OnceCell;
use std::hint::black_box;

use criterion::{BenchmarkId, Criterion, Throughput};

use crate::bench::fixture::{Kind, Scale};
use crate::bench::scene::{FRAME, Field};
use crate::simd::Kernel;
use crate::simd::isa_tier::IsaTier;
use crate::star_detection::bench::Stages;
use crate::star_detection::threshold_mask::simd::{ProcessWords, WORD_PIXELS};
use crate::star_detection::threshold_mask::threshold_params::ThresholdParams;

pub(crate) fn atomic(c: &mut Criterion) {
    let stages = OnceCell::new();
    let row = || {
        let stages: &Stages = stages.get_or_init(|| Stages::of(Field::Typical));
        let y = FRAME.height / 2;
        let threshold = ThresholdParams {
            sigma: stages.config.extraction.sigma_threshold,
            min_noise: stages.plane.noise.floor,
        };
        Row {
            pixels: stages.plane.values.row(y),
            noise: stages.plane.noise.noise.reader().row(y).to_vec(),
            threshold,
        }
    };
    let tiers: Vec<IsaTier> = IsaTier::supported().collect();
    let mut group = Kind::Atomic.group(c, "detection/threshold_mask", Scale::Sample);
    group.throughput(Throughput::Elements(FRAME.width as u64));
    let mut words = vec![0; FRAME.width.div_ceil(WORD_PIXELS)];
    group.bench_function(BenchmarkId::new("row/scalar", FRAME.width), |b| {
        let Row {
            pixels,
            noise,
            threshold,
        } = row();
        b.iter(|| {
            ProcessWords {
                pixels: black_box(pixels),
                noise: &noise,
                threshold,
                words: &mut words,
            }
            .run_scalar();
        });
    });
    for &tier in &tiers {
        group.bench_function(
            BenchmarkId::new(format!("row/{}", tier.id()), FRAME.width),
            |b| {
                let Row {
                    pixels,
                    noise,
                    threshold,
                } = row();
                b.iter(|| {
                    tier.run(ProcessWords {
                        pixels: black_box(pixels),
                        noise: &noise,
                        threshold,
                        words: &mut words,
                    });
                });
            },
        );
    }
    group.bench_function(BenchmarkId::new("row/dispatch", FRAME.width), |b| {
        let Row {
            pixels,
            noise,
            threshold,
        } = row();
        b.iter(|| {
            ProcessWords {
                pixels: black_box(pixels),
                noise: &noise,
                threshold,
                words: &mut words,
            }
            .dispatch();
        });
    });
}

/// One row of the plane, its noise read off the plane's surface, and the threshold the detector
/// puts on it.
#[derive(Debug)]
struct Row<'a> {
    pixels: &'a [f32],
    noise: Vec<f32>,
    threshold: ThresholdParams,
}
