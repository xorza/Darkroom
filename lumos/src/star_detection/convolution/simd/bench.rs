//! The matched filter's row pass over one row of a frame's residual, at the FWHM the frame's stars
//! give, on the scalar reference, every tier's kernel and the dispatcher: production convolves the
//! rows on the global pool, a row a task.

use std::cell::OnceCell;
use std::hint::black_box;

use criterion::{BenchmarkId, Criterion, Throughput};

use crate::bench::fixture::{self, Scale};
use crate::bench::scene::{FRAME, Field};
use crate::math::fwhm::fwhm_to_sigma;
use crate::simd::tier::Tier;
use crate::star_detection::bench::Stages;
use crate::star_detection::convolution::simd::internals::convolve_row_scalar;
use crate::star_detection::convolution::simd::{ConvolveRow, convolve_row};
use crate::star_detection::convolution::{fill_gaussian_kernel_1d, kernel_radius};

pub(crate) fn benches(c: &mut Criterion) {
    let stages = OnceCell::new();
    let inputs = || {
        let stages: &Stages = stages.get_or_init(|| Stages::of(Field::Typical));
        let sigma = fwhm_to_sigma(stages.fwhm.expect("the default configuration filters"));
        let mut kernel = Vec::new();
        fill_gaussian_kernel_1d(sigma, kernel_radius(sigma), &mut kernel);
        let y = FRAME.height / 2;
        let row = &stages.frame.measure.pixels()[y * FRAME.width..(y + 1) * FRAME.width];
        Inputs { row, kernel }
    };
    let tiers: Vec<Tier> = Tier::supported().collect();
    let mut group = fixture::group(c, "kernel/detection/convolution", Scale::Sample);
    group.throughput(Throughput::Elements(FRAME.width as u64));
    let mut output = vec![0.0; FRAME.width];
    group.bench_function(BenchmarkId::new("row/scalar", FRAME.width), |b| {
        let Inputs { row, kernel } = inputs();
        b.iter(|| convolve_row_scalar(black_box(row), &mut output, &kernel));
    });
    for &tier in &tiers {
        group.bench_function(BenchmarkId::new(format!("row/{tier}"), FRAME.width), |b| {
            let Inputs { row, kernel } = inputs();
            b.iter(|| {
                tier.run(ConvolveRow {
                    input: black_box(row),
                    output: &mut output,
                    kernel: &kernel,
                });
            });
        });
    }
    group.bench_function(BenchmarkId::new("row/dispatch", FRAME.width), |b| {
        let Inputs { row, kernel } = inputs();
        b.iter(|| convolve_row(black_box(row), &mut output, &kernel));
    });
}

/// One row of the residual and the matched filter's 1D kernel.
#[derive(Debug)]
struct Inputs<'a> {
    row: &'a [f32],
    kernel: Vec<f32>,
}
