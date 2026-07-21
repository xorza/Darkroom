//! Per-op benchmarks for the image operations, run against the bundled real stacked light master
//! (`test_data/lumos_data/stacked_light.tiff` — a 279 MB TIFF, ~288 MB as an RGB-f32 `Image`). Each
//! op is timed on the input domain it's actually used in: the linear-domain ops (stretch, background
//! neutralization, denoise) on the raw linear master, the display enhancers (SCNR, background
//! extraction, HDR, local contrast) on the standard stretched `[0, 1]` master. The ML ops are not
//! benched here — they need the `ml` feature and caller-supplied ONNX weights, which lumos doesn't
//! ship.
//!
//! Each op's `apply` mutates in place, so a fresh master is needed per iteration. Cloning this 288 MB
//! buffer costs ~260 ms (malloc + first-touch page-faulting, not the op), so the clones are made
//! *outside* the timed region — [`bench_op`] pre-builds exactly one per closure call — and the
//! reported numbers are the net op cost.
//!
//! Gated behind the `real-data` feature (the dataset is). Run:
//! `cargo test -p lumos --release --features real-data image_ops::bench -- --ignored --nocapture`

use quickbench::quick_bench;
use std::hint::black_box;

use imaginarium::Image;

use crate::io::image::linear::LinearImage;
use crate::io::image::LoadContext;
use crate::testing::calibration_dir;
use crate::{Denoise, ExtractBackground, Hdr, LocalContrast, NeutralizeBackground, Scnr, Stretch};

// Must match the `#[quick_bench]` attributes below: quickbench's iter-capped loops call the closure
// exactly `WARMUP_ITERS + ITERS` times, which is the pre-clone pool size.
const WARMUP_ITERS: usize = 1;
const ITERS: usize = 5;

/// The bundled stacked light master as a linear f32 `Image` — the input the linear-domain ops
/// receive (a real stack, so its bright star cores exceed 1.0).
fn linear_master() -> Image {
    Image::from(
        &LinearImage::from_file(
            calibration_dir().join("stacked_light.tiff"),
            &LoadContext::default(),
        )
            .expect("load stacked_light.tiff"),
    )
}

/// A display-domain `[0, 1]` master: the standard prep (neutralize → STF stretch → SCNR) that the
/// display enhancers run on.
fn display_master() -> Image {
    let mut img = linear_master();
    NeutralizeBackground.apply(&mut img).unwrap();
    Stretch::auto_stf().apply(&mut img).unwrap();
    Scnr::average_neutral().apply(&mut img).unwrap();
    img
}

/// Time `op` on a fresh copy of `master` each iteration, with the copies cloned *before* the timed
/// region so the ~260 ms clone of this 288 MB master stays out of the measurement. The pool is sized
/// to quickbench's exact call count; the `pop` fallback clones if that ever drifts.
fn bench_op(b: ::quickbench::Bencher, master: &Image, op: impl Fn(&mut Image)) {
    let mut pool: Vec<Image> = (0..WARMUP_ITERS + ITERS).map(|_| master.clone()).collect();
    b.bench(move || {
        let mut img = pool.pop().unwrap_or_else(|| master.clone());
        op(&mut img);
        black_box(img)
    });
}

#[quick_bench(warmup_iters = 1, iters = 5)]
fn bench_stretch_auto_stf(b: ::quickbench::Bencher) {
    let master = linear_master();
    bench_op(b, &master, |img| Stretch::auto_stf().apply(img).unwrap());
}

#[quick_bench(warmup_iters = 1, iters = 5)]
fn bench_stretch_auto_asinh(b: ::quickbench::Bencher) {
    let master = linear_master();
    bench_op(b, &master, |img| Stretch::auto_asinh().apply(img).unwrap());
}

#[quick_bench(warmup_iters = 1, iters = 5)]
fn bench_neutralize_background(b: ::quickbench::Bencher) {
    let master = linear_master();
    bench_op(b, &master, |img| NeutralizeBackground.apply(img).unwrap());
}

#[quick_bench(warmup_iters = 1, iters = 5)]
fn bench_denoise(b: ::quickbench::Bencher) {
    let master = linear_master();
    bench_op(b, &master, |img| Denoise::default().apply(img).unwrap());
}

#[quick_bench(warmup_iters = 1, iters = 5)]
fn bench_scnr(b: ::quickbench::Bencher) {
    let master = display_master();
    bench_op(b, &master, |img| {
        Scnr::average_neutral().apply(img).unwrap()
    });
}

#[quick_bench(warmup_iters = 1, iters = 5)]
fn bench_extract_background(b: ::quickbench::Bencher) {
    let master = display_master();
    bench_op(b, &master, |img| {
        ExtractBackground::default().apply(img).unwrap()
    });
}

#[quick_bench(warmup_iters = 1, iters = 5)]
fn bench_hdr(b: ::quickbench::Bencher) {
    let master = display_master();
    bench_op(b, &master, |img| Hdr::default().apply(img).unwrap());
}

#[quick_bench(warmup_iters = 1, iters = 5)]
fn bench_local_contrast(b: ::quickbench::Bencher) {
    let master = display_master();
    bench_op(b, &master, |img| {
        LocalContrast::default().apply(img).unwrap()
    });
}
