//! Benchmarks for drizzle reconstruction (Fruchter & Hook), on synthetic dithered frames.
//!
//! Two sweeps, each covering one axis completely rather than one hand-written benchmark per
//! combination:
//!
//! - [`bench_drizzle_kernels`] — every kernel, on a dither-only field and on one rotated by
//!   [`ROTATION_DEGREES`]. The per-frame flux distribution is the expensive part and it differs by
//!   kernel: Turbo (axis-aligned box) vs Square (exact polygon clipping) vs the radial pair
//!   (Gaussian / Lanczos, a normalized tap grid) vs Point (one output pixel).
//! - [`bench_drizzle_quality_planes`] — what the ancillary planes cost the scatter.
//!
//! The rotation is there because the scatter is parallelized over output bands, and a band
//! inverse-maps to a strip of the input that is only axis-aligned while the transform is: rotate by θ
//! and a band `h` output rows tall spanning `W` columns takes in `h·cosθ + W·sinθ` rows' worth of
//! input, nearly all of it rejected. A translation-only fixture cannot see that at all.
//!
//! **Read the pairs, not the absolutes.** This machine's clocks drift far wider than the differences
//! being measured — an unchanged binary has run 40% apart across one session — so a case is only
//! comparable to one measured beside it, which is why the whole table lives in one sweep. Each case
//! is given a wall-clock window rather than an iteration count for the same reason: a 40 ms case run
//! three times samples 120 ms of whatever the governor was doing, and it reported swings of ±40%
//! and even a negative rotation cost until it was measured over a second like every other case.
//!
//! Run: `cargo test -p lumos --release drizzle::bench -- --ignored --nocapture`

use crate::testing::prelude::*;
use quickbench::quick_bench;
use std::hint::black_box;

use crate::stacking::drizzle::accumulator::DrizzleFrame;
use crate::stacking::drizzle::config::{DrizzleConfig, DrizzleKernel};
use crate::stacking::drizzle::stack::drizzle_images;
use crate::stacking::progress::ProgressCallback;
use crate::stacking::registration::transform::Transform;
use crate::stacking::stack_product::StackProduct;
use crate::stacking::stack_product::quality_planes::QualityPlanes;
use crate::testing::synthetic::fixtures::star_field;

const N_FRAMES: usize = 8;
const FIELD: Size2us = Size2us::new(1000, 1000);
/// Field rotation of the rotated leg. A degree is a mild night's drift for an unguided set, and on
/// this 2000-column output grid it widens a band's input strip by 35 output rows — enough that the
/// over-scan is measured rather than inferred.
const ROTATION_DEGREES: f64 = 1.0;
/// Every kernel, so the sweep is the whole table and not the three that were interesting once.
///
/// Cheapest first: the sweep is half a minute of saturated multi-thread work and the tail runs at a
/// lower clock than the head, so the rows where a fixed overhead is the largest share are the ones
/// measured coolest.
const KERNELS: [DrizzleKernel; 5] = [
    DrizzleKernel::Point,
    DrizzleKernel::Turbo,
    DrizzleKernel::Square,
    DrizzleKernel::Gaussian,
    DrizzleKernel::Lanczos,
];
/// The geometries every kernel is measured on, labelled as they are reported.
const GEOMETRIES: [(&str, f64); 2] = [("aligned", 0.0), ("rotated", ROTATION_DEGREES)];

/// [`N_FRAMES`] copies of one synthetic field, each with a small sub-pixel dither and `rotation`
/// radians about the field centre — the input a drizzle integration sees.
///
/// The dither is the same sequence whatever the rotation, and a rotation of zero composes to exactly
/// the translation, so the two geometries differ in one thing only.
fn dithered_set(base: &LinearImage, rotation: f64) -> Vec<DrizzleFrame<LinearImage>> {
    let centre = DVec2::new(FIELD.width as f64, FIELD.height as f64) / 2.0;
    (0..N_FRAMES)
        .map(|i| {
            let dx = (i as f64 * 0.37).fract() * 2.0 - 1.0;
            let dy = (i as f64 * 0.71).fract() * 2.0 - 1.0;
            let transform = Transform::translation(DVec2::new(dx, dy))
                .compose(&Transform::rotation_around(centre, rotation));
            DrizzleFrame::new(base.clone(), transform)
        })
        .collect()
}

/// The output grid and drop size a kernel is benched at.
///
/// The realistic drizzle settings, except for Lanczos: its own config validation restricts it to
/// scale 1 / pixfrac 1, so its row of the table is a quarter of the output grid the others build and
/// is comparable only to itself.
fn kernel_config(kernel: DrizzleKernel) -> DrizzleConfig {
    let (scale, pixfrac) = match kernel {
        DrizzleKernel::Lanczos => (1.0, 1.0),
        _ => (2.0, 0.8),
    };
    DrizzleConfig {
        scale,
        pixfrac,
        kernel,
        quality: QualityPlanes::ALL,
        ..DrizzleConfig::default()
    }
}

/// One drizzle of the whole set, which is what every case below measures.
///
/// `drizzle_images` consumes its frames — the streaming entry point exists so a decoded frame is
/// dropped as soon as it is distributed — so each iteration has to hand it a fresh set. That clone is
/// inside the measurement and the `frame-clone` case is what it costs.
///
/// The result is unwrapped rather than black-boxed as a `Result`: a rejected config returns in
/// nanoseconds, and a bench that reports that as a fast drizzle is worse than no bench.
fn drizzle(frames: &[DrizzleFrame<LinearImage>], config: &DrizzleConfig) -> StackProduct {
    drizzle_images(
        frames.to_vec(),
        config,
        ProgressCallback::default(),
        CancelToken::never(),
    )
    .expect("bench fixture must drizzle")
}

/// Every kernel on both geometries, plus the fixture clone every one of them pays.
///
/// One sweep rather than a benchmark per combination: the kernel and the field geometry are the two
/// axes the scatter's cost turns on, and reading either off needs the other held fixed in the same
/// process — the run-to-run drift on this machine is far wider than the differences being measured.
#[quick_bench(warmup_time_ms = 200, bench_time_ms = 1000)]
fn bench_drizzle_kernels(b: ::quickbench::Bencher) {
    let base = star_field(FIELD, 250, 5).image;
    let aligned = dithered_set(&base, 0.0);

    b.bench_labeled("frame-clone", || black_box(aligned.to_vec()));

    for kernel in KERNELS {
        let config = kernel_config(kernel);
        for (geometry, degrees) in GEOMETRIES {
            let frames = dithered_set(&base, degrees.to_radians());
            b.bench_labeled(&format!("{kernel:?}/{geometry}"), || {
                black_box(drizzle(&frames, &config))
            });
        }
    }
}

/// What `DrizzleConfig::quality` costs, on the default kernel.
///
/// Declining every plane drops the `Σwᵢ²` accumulator resident over the run, two arithmetic
/// operations and a bitmap test at every deposit, and two output-grid planes never built.
#[quick_bench(warmup_time_ms = 200, bench_time_ms = 1000)]
fn bench_drizzle_quality_planes(b: ::quickbench::Bencher) {
    let frames = dithered_set(&star_field(FIELD, 250, 5).image, 0.0);
    for quality in [QualityPlanes::ALL, QualityPlanes::IMAGE_ONLY] {
        let config = DrizzleConfig {
            quality,
            ..kernel_config(DrizzleKernel::Turbo)
        };
        let label = if quality == QualityPlanes::ALL {
            "all-planes"
        } else {
            "image-only"
        };
        b.bench_labeled(label, || black_box(drizzle(&frames, &config)));
    }
}
