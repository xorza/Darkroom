//! The matched filter's row pass over one row of a frame's residual, at the FWHM the frame's stars
//! give, on the scalar reference, every tier's kernel and the dispatcher: production convolves the
//! rows on the global pool, a row a task.

use std::cell::OnceCell;
use std::hint::black_box;

use criterion::{BenchmarkId, Criterion, Throughput};

use crate::bench::fixture::{Kind, Scale};
use crate::bench::scene::{FRAME, Field};
use crate::math::fwhm;
use crate::simd::Kernel;
use crate::simd::isa_tier::IsaTier;
use crate::star_detection::bench::Stages;
use crate::star_detection::convolution;
use crate::star_detection::convolution::simd::ConvolveRow;

pub(crate) fn atomic(c: &mut Criterion) {
    let stages = OnceCell::new();
    let inputs = || {
        let stages: &Stages = stages.get_or_init(|| Stages::of(Field::Typical));
        let sigma = fwhm::to_sigma(stages.fwhm.expect("the default configuration filters"));
        let mut kernel = Vec::new();
        convolution::fill_gaussian_kernel_1d(sigma, convolution::kernel_radius(sigma), &mut kernel);
        let y = FRAME.height / 2;
        let row = &stages.frame.measure.pixels()[y * FRAME.width..(y + 1) * FRAME.width];
        Inputs { row, kernel }
    };
    let tiers: Vec<IsaTier> = IsaTier::supported().collect();
    let mut group = Kind::Atomic.group(c, "detection/convolution", Scale::Sample);
    group.throughput(Throughput::Elements(FRAME.width as u64));
    let mut output = vec![0.0; FRAME.width];
    group.bench_function(BenchmarkId::new("row/scalar", FRAME.width), |b| {
        let Inputs { row, kernel } = inputs();
        b.iter(|| {
            ConvolveRow {
                input: black_box(row),
                output: &mut output,
                kernel: &kernel,
            }
            .run_scalar();
        });
    });
    for &tier in &tiers {
        group.bench_function(
            BenchmarkId::new(format!("row/{}", tier.id()), FRAME.width),
            |b| {
                let Inputs { row, kernel } = inputs();
                b.iter(|| {
                    tier.run(ConvolveRow {
                        input: black_box(row),
                        output: &mut output,
                        kernel: &kernel,
                    });
                });
            },
        );
    }
    group.bench_function(BenchmarkId::new("row/dispatch", FRAME.width), |b| {
        let Inputs { row, kernel } = inputs();
        b.iter(|| {
            ConvolveRow {
                input: black_box(row),
                output: &mut output,
                kernel: &kernel,
            }
            .dispatch();
        });
    });
}

/// One row of the residual and the matched filter's 1D kernel.
#[derive(Debug)]
struct Inputs<'a> {
    row: &'a [f32],
    kernel: Vec<f32>,
}
