//! Per-op benchmarks for the image operations, run against the bundled real stacked light master
//! (`test_data/lumos_data/stacked_light.tiff` — a 279 MB TIFF, ~288 MB as an RGB-f32 `Image`). Each
//! op is timed on the input domain it's actually used in: the linear-domain ops (stretch,
//! background neutralization, denoise) on the raw linear master, the display enhancers (SCNR,
//! background extraction, HDR, local contrast) on the standard stretched `[0, 1]` master. The ML
//! ops are not benched here — they need the `ml` feature and caller-supplied ONNX weights, which
//! lumos doesn't ship.
//!
//! Each op's `apply` mutates in place, so a fresh master is needed per iteration. Cloning this 288
//! MB buffer costs ~260 ms (malloc + first-touch page-faulting, not the op), so the clones are made
//! *outside* the timed region — [`bench_op`] pre-builds exactly one per closure call — and the
//! reported numbers are the net op cost.
//!
//! Gated behind the `real-data` feature (the dataset is). Run: `cargo test -p lumos --release
//! --features bench,real-data image_ops::bench -- --ignored --nocapture`

use crate::internals::prelude::*;
use quickbench::quick_bench;
use std::hint::black_box;

use crate::internals::real_data;
use crate::{
    ColorMode, Denoise, ExtractBackground, Hdr, LocalContrast, NeutralizeBackground, Scnr, Stretch,
    StretchMethod,
};

// Must match the `#[quick_bench]` attributes below: quickbench's iter-capped loops call the closure
// exactly `WARMUP_ITERS + ITERS` times, which is the pre-clone pool size.
const WARMUP_ITERS: usize = 1;
const ITERS: usize = 5;

/// Time `op` on a fresh copy of `master` each iteration, with the copies cloned *before* the timed
/// region so the ~260 ms clone of this 288 MB master stays out of the measurement. The pool is
/// sized to quickbench's exact call count; the `pop` fallback clones if that ever drifts.
fn bench_op(b: ::quickbench::Bencher, master: &LinearImage, op: impl Fn(&mut LinearImage)) {
    let mut pool: Vec<LinearImage> = (0..WARMUP_ITERS + ITERS).map(|_| master.clone()).collect();
    b.bench(move || {
        let mut img = pool.pop().unwrap_or_else(|| master.clone());
        op(&mut img);
        black_box(img)
    });
}

#[quick_bench(warmup_iters = 1, iters = 5)]
fn bench_stretch_auto_stf(b: ::quickbench::Bencher) {
    let master = real_data::linear_master();
    bench_op(b, &master, |img| Stretch::auto_stf().apply(img).unwrap());
}

#[quick_bench(warmup_iters = 1, iters = 5)]
fn bench_stretch_auto_asinh(b: ::quickbench::Bencher) {
    let master = real_data::linear_master();
    bench_op(b, &master, |img| Stretch::auto_asinh().apply(img).unwrap());
}

/// The same AVX2 arcsinh kernel as [`bench_stretch_auto_asinh`] with the auto prologue removed: an
/// explicit `beta` skips `subsample_intensity` and the median/MAD curve fit, so the difference
/// between the two is what that prologue costs on a full-size master, and this number alone is the
/// kernel plus its memory traffic. Isolated because `stretching::bench`'s kernel bench runs
/// single-threaded on a 6 MP synthetic image, which says nothing about either at 24 MP.
#[quick_bench(warmup_iters = 1, iters = 5)]
fn bench_stretch_asinh_explicit(b: ::quickbench::Bencher) {
    let master = real_data::linear_master();
    bench_op(b, &master, |img| {
        Stretch {
            method: StretchMethod::Asinh {
                black_point: 0.0,
                beta: 0.05,
            },
            color: ColorMode::ColorPreserving,
        }
        .apply(img)
        .unwrap();
    });
}

#[quick_bench(warmup_iters = 1, iters = 5)]
fn bench_neutralize_background(b: ::quickbench::Bencher) {
    let master = real_data::linear_master();
    bench_op(b, &master, |img| NeutralizeBackground.apply(img).unwrap());
}

#[quick_bench(warmup_iters = 1, iters = 5)]
fn bench_denoise(b: ::quickbench::Bencher) {
    let master = real_data::linear_master();
    bench_op(b, &master, |img| Denoise::default().apply(img).unwrap());
}

#[quick_bench(warmup_iters = 1, iters = 5)]
fn bench_scnr(b: ::quickbench::Bencher) {
    let master = real_data::display_master();
    bench_op(b, &master, |img| {
        Scnr::average_neutral(1.0).apply(img).unwrap();
    });
}

#[quick_bench(warmup_iters = 1, iters = 5)]
fn bench_extract_background(b: ::quickbench::Bencher) {
    let master = real_data::display_master();
    bench_op(b, &master, |img| {
        ExtractBackground::default().apply(img).unwrap();
    });
}

#[quick_bench(warmup_iters = 1, iters = 5)]
fn bench_hdr(b: ::quickbench::Bencher) {
    let master = real_data::display_master();
    bench_op(b, &master, |img| Hdr::default().apply(img).unwrap());
}

#[quick_bench(warmup_iters = 1, iters = 5)]
fn bench_local_contrast(b: ::quickbench::Bencher) {
    let master = real_data::display_master();
    bench_op(b, &master, |img| {
        LocalContrast::default().apply(img).unwrap();
    });
}
