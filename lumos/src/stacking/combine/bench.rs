//! Benchmarks for the in-memory combine engine (`stack_images`), isolated from RAW decode.
//!
//! Builds a synthetic light-frame set in memory and stacks it, so the measured time is the combine
//! hot path: normalization, weight resolution, per-pixel rejection, and weighted accumulation.
//!
//! Run: `cargo test -p lumos --release combine::bench -- --ignored --nocapture`

use common::CancelToken;
use quickbench::quick_bench;
use std::hint::black_box;

use crate::LinearImage;
use crate::io::astro_image::ImageDimensions;
use crate::stacking::combine::config::StackConfig;
use crate::stacking::combine::stack::{StackFrame, stack_images};
use crate::stacking::progress::ProgressCallback;

/// A 1 MP mono frame: smooth background + per-frame offset/gain (so normalization has work to do) +
/// ~0.2% bright outliers (so rejection has something to clip).
fn synth_frame(w: usize, h: usize, frame: u32) -> LinearImage {
    let n = w * h;
    let offset = 0.05 + (frame as f32) * 0.002;
    let gain = 1.0 + (frame as f32) * 0.01;
    let mut px = vec![0.0f32; n];
    for (i, p) in px.iter_mut().enumerate() {
        let hash = (i as u32).wrapping_mul(2654435761) ^ frame.wrapping_mul(40503);
        let noise = (hash as f32 / u32::MAX as f32 - 0.5) * 0.02;
        *p = (0.2 + noise) * gain + offset;
    }
    for k in 0..(n / 500) {
        let idx = ((k as u32).wrapping_mul(2246822519) ^ frame) as usize % n;
        px[idx] = 0.95;
    }
    LinearImage::from_planar_channels(ImageDimensions::new((w, h), 1), [px])
}

const W: usize = 1024;
const H: usize = 1024;
const FRAMES: u32 = 30;

fn frames() -> Vec<StackFrame> {
    (0..FRAMES).map(|f| synth_frame(W, H, f).into()).collect()
}

fn run(config: StackConfig) {
    let result = stack_images(
        frames(),
        config,
        ProgressCallback::default(),
        CancelToken::never(),
    )
    .unwrap();
    black_box(result);
}

/// Science default: σ-clipped mean (2.5σ) + noise weighting + global normalization.
#[quick_bench(warmup_iters = 1, iters = 5)]
fn bench_stack_light_30(b: ::quickbench::Bencher) {
    b.bench(|| run(StackConfig::light()));
}

/// Median combine — the rejection-free baseline (per-pixel quickselect, no iteration).
#[quick_bench(warmup_iters = 1, iters = 5)]
fn bench_stack_median_30(b: ::quickbench::Bencher) {
    b.bench(|| run(StackConfig::median()));
}

/// Winsorized σ-clip — the small-stack-stable rejection used by dark/bias masters.
#[quick_bench(warmup_iters = 1, iters = 5)]
fn bench_stack_winsorized_30(b: ::quickbench::Bencher) {
    b.bench(|| run(StackConfig::winsorized(3.0)));
}
