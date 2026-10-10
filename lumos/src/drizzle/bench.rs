//! Benchmarks for drizzle reconstruction (Fruchter & Hook), on synthetic dithered frames.
//!
//! Two sweeps, each covering one axis completely rather than one hand-written benchmark per
//! combination:
//!
//! - [`bench_drizzle_kernels`] — the scatter alone, every kernel on every geometry of
//!   [`GEOMETRIES`]. The per-frame flux distribution is the expensive part and it differs by
//!   kernel: Turbo (axis-aligned box) vs Square (exact polygon clipping) vs the radial pair
//!   (Gaussian / Lanczos, a normalized tap grid) vs Point (one output pixel).
//! - [`bench_drizzle_quality_planes`] — what the combine's quality planes cost a whole drizzle.
//!
//! The geometries are there because the scatter is parallelized over output bands, and a band
//! inverse-maps to a strip of the input that is only axis-aligned while the transform is: rotate by
//! θ and a band `h` output rows tall spanning `W` columns takes in `h·cosθ + W·sinθ` rows' worth of
//! input, nearly all of it rejected unless the scan limits each row to the columns that reach the
//! band. A translation-only fixture cannot see that at all; 90° is the worst case, and a SIP map is
//! the one whose outline is sampled rather than taken in closed form.
//!
//! **Read the pairs, not the absolutes.** This machine's clocks drift far wider than the
//! differences being measured — an unchanged binary has run 40% apart across one session — so a
//! case is only comparable to one measured beside it, which is why the whole table lives in one
//! sweep. Each case is given a wall-clock window rather than an iteration count for the same
//! reason: a 40 ms case run three times samples 120 ms of whatever the governor was doing, and it
//! reported swings of ±40% and even a negative rotation cost until it was measured over a second
//! like every other case.
//!
//! Run: `cargo test -p lumos --release --features bench drizzle::bench -- --ignored --nocapture`

use crate::internals::prelude::*;
use quickbench::quick_bench;
use std::hint::black_box;

use crate::combine::config::{Combine, Normalization, StackConfig, Weighting};
use crate::drizzle::accumulator::{DrizzleAccumulator, DrizzleFrame};
use crate::drizzle::config::{DrizzleConfig, DrizzleKernel};
use crate::drizzle::deposit::Deposit;
use crate::drizzle::stack::drizzle_images;
use crate::internals::synthetic::fixtures::star_field;
use crate::progress::progress_callback::ProgressCallback;
use crate::registration::distortion::sip::SipPolynomial;
use crate::registration::transform::{Transform, WarpTransform};
use crate::stack_product::StackProduct;
use crate::stack_product::quality_planes::QualityPlanes;

const N_FRAMES: usize = 8;
const FIELD: Size2us = Size2us::new(1000, 1000);
/// Every kernel at its usual pixfrac, so the sweep is the whole table and not the three that were
/// interesting once, and the square kernel again at pixfrac 1, where neighbouring drops share
/// their corners.
///
/// Cheapest first: the sweep is half a minute of saturated multi-thread work and the tail runs at a
/// lower clock than the head, so the rows where a fixed overhead is the largest share are the ones
/// measured coolest.
const KERNELS: [(DrizzleKernel, f32); 6] = [
    (DrizzleKernel::Point, 0.8),
    (DrizzleKernel::Turbo, 0.8),
    (DrizzleKernel::Square, 0.8),
    (DrizzleKernel::Square, 1.0),
    (DrizzleKernel::Gaussian, 0.8),
    (DrizzleKernel::Lanczos, 1.0),
];

/// How the frames lie on the reference: a field rotation in degrees, and whether a SIP field
/// bends it.
#[derive(Debug, Clone, Copy)]
struct Geometry {
    degrees: f64,
    sip: bool,
}

/// The geometries every kernel is measured on, labelled as they are reported: a degree is a mild
/// night's drift for an unguided set, 45° and 90° a session rotated by its operator, and the SIP
/// leg a degree under a fitted radial field.
const GEOMETRIES: [(&str, Geometry); 5] = [
    (
        "aligned",
        Geometry {
            degrees: 0.0,
            sip: false,
        },
    ),
    (
        "rotated",
        Geometry {
            degrees: 1.0,
            sip: false,
        },
    ),
    (
        "45deg",
        Geometry {
            degrees: 45.0,
            sip: false,
        },
    ),
    (
        "90deg",
        Geometry {
            degrees: 90.0,
            sip: false,
        },
    ),
    (
        "sip",
        Geometry {
            degrees: 1.0,
            sip: true,
        },
    ),
];

/// [`N_FRAMES`] copies of one synthetic field, each with a small sub-pixel dither and `geometry`'s
/// rotation about the field centre — the input a drizzle integration sees.
///
/// The dither is the same sequence whatever the geometry, and a rotation of zero composes to
/// exactly the translation, so two geometries differ in what they name only.
fn dithered_set(base: &LinearImage, geometry: Geometry) -> Vec<DrizzleFrame<LinearImage>> {
    let centre = DVec2::new(FIELD.width as f64, FIELD.height as f64) / 2.0;
    (0..N_FRAMES)
        .map(|i| {
            let dx = (i as f64 * 0.37).fract() * 2.0 - 1.0;
            let dy = (i as f64 * 0.71).fract() * 2.0 - 1.0;
            let transform = Transform::translation(DVec2::new(dx, dy)).compose(
                &Transform::rotation_around(centre, geometry.degrees.to_radians()),
            );
            let warp = if geometry.sip {
                sip_warp(transform.inverse(), centre)
            } else {
                WarpTransform::new(transform.inverse())
            };
            DrizzleFrame::new(base.clone(), warp)
        })
        .collect()
}

/// `transform` under a fitted radial SIP field of a few pixels at the corners, the warp a wide
/// field registers to.
fn sip_warp(transform: Transform, centre: DVec2) -> WarpTransform {
    let field = |d: DVec2| d * 1e-6 * d.length_squared();
    let reference: Vec<DVec2> = (0..FIELD.height)
        .step_by(25)
        .flat_map(|y| {
            (0..FIELD.width)
                .step_by(25)
                .map(move |x| DVec2::new(x as f64, y as f64))
        })
        .collect();
    let target: Vec<DVec2> = reference
        .iter()
        .map(|&r| transform.apply(r + field(r - centre)))
        .collect();
    let sip = SipPolynomial::fitted_under(&transform, &reference, &target, 3, centre);
    WarpTransform::with_sip(transform, sip)
}

/// The output grid a kernel is benched at, with drops of `pixfrac`.
///
/// Scale 2, except for Lanczos: its own config validation restricts it to scale 1 / pixfrac 1, so
/// its row of the table is a quarter of the output grid the others build and is comparable only to
/// itself.
fn kernel_config(kernel: DrizzleKernel, pixfrac: f32) -> DrizzleConfig {
    let scale = match kernel {
        DrizzleKernel::Lanczos => 1.0,
        _ => 2.0,
    };
    DrizzleConfig {
        scale,
        pixfrac,
        kernel,
        ..DrizzleConfig::default()
    }
}

/// One drizzle of the whole set, scatter and combine, which the quality-plane sweep measures.
///
/// `drizzle_images` consumes its frames — the streaming entry point exists so a decoded frame is
/// dropped as soon as it is distributed — so each iteration has to hand it a fresh set.
///
/// The result is unwrapped rather than black-boxed as a `Result`: a rejected config returns in
/// nanoseconds, and a bench that reports that as a fast drizzle is worse than no bench.
fn drizzle(
    frames: &[DrizzleFrame<LinearImage>],
    config: &DrizzleConfig,
    quality: QualityPlanes,
) -> StackProduct {
    drizzle_images(
        frames.to_vec(),
        config,
        &StackConfig {
            combine: Combine::mean(),
            weighting: Weighting::Equal,
            normalization: Normalization::None,
            quality,
            ..StackConfig::light()
        },
        ProgressCallback::default(),
        CancelToken::never(),
    )
    .expect("bench fixture must drizzle")
    .product
}

/// Every frame of `frames` scattered into one accumulator: the scatter alone, with no combine.
fn scatter(frames: &[DrizzleFrame<LinearImage>], config: &DrizzleConfig) -> usize {
    let mut accumulator = DrizzleAccumulator::new(
        ImageDimensions::new(FIELD, 1),
        Deposit::Channels(1),
        config.clone(),
        0,
    )
    .expect("bench config must be valid");
    for frame in frames {
        accumulator
            .add_frame(frame)
            .expect("bench fixture must drizzle");
    }
    accumulator.unconverged_points()
}

/// Every kernel on every geometry, the scatter alone.
///
/// One sweep rather than a benchmark per combination: the kernel and the field geometry are the two
/// axes the scatter's cost turns on, and reading either off needs the other held fixed in the same
/// process — the run-to-run drift on this machine is far wider than the differences being measured.
#[quick_bench(warmup_time_ms = 200, bench_time_ms = 1000)]
fn bench_drizzle_kernels(b: ::quickbench::Bencher) {
    let base = star_field(FIELD, 250, 5).image;
    for (kernel, pixfrac) in KERNELS {
        let config = kernel_config(kernel, pixfrac);
        for (label, geometry) in GEOMETRIES {
            let frames = dithered_set(&base, geometry);
            b.bench_labeled(&format!("{kernel:?}-p{pixfrac}/{label}"), || {
                black_box(scatter(&frames, &config))
            });
        }
    }
}

/// What the combine's quality planes cost a drizzle, on the default kernel.
#[quick_bench(warmup_time_ms = 200, bench_time_ms = 1000)]
fn bench_drizzle_quality_planes(b: ::quickbench::Bencher) {
    let frames = dithered_set(&star_field(FIELD, 250, 5).image, GEOMETRIES[0].1);
    let config = kernel_config(DrizzleKernel::Turbo, 0.8);
    for quality in [QualityPlanes::STANDARD, QualityPlanes::IMAGE_ONLY] {
        let label = if quality == QualityPlanes::STANDARD {
            "all-planes"
        } else {
            "image-only"
        };
        b.bench_labeled(label, || black_box(drizzle(&frames, &config, quality)));
    }
}
