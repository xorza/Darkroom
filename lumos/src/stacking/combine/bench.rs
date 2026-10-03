//! Benchmarks for the in-memory combine engine (`stack_images`), isolated from RAW decode.
//!
//! Builds a synthetic light-frame set in memory once and stacks copies of it, so the measured time
//! is the combine hot path — normalization, weight resolution, per-pixel rejection and weighted
//! accumulation — plus a clone of the set, which a separate row reports.
//!
//! Run: `cargo test -p lumos --release --features bench combine::bench -- --ignored --nocapture`

use crate::testing::prelude::*;
use quickbench::quick_bench;
use std::hint::black_box;

use crate::stacking::combine::config::StackConfig;
use crate::stacking::combine::stack::{StackFrame, stack_images};
use crate::stacking::progress::ProgressCallback;

/// A 1 MP mono frame: smooth background + per-frame offset/gain (so normalization has work to do) +
/// ~0.2% bright outliers (so rejection has something to clip).
fn synth_frame(size: Size2us, frame: u32) -> LinearImage {
    let n = size.pixel_count();
    let offset = 0.05 + (frame as f32) * 0.002;
    let gain = 1.0 + (frame as f32) * 0.01;
    let mut rng = TestRng::new(u64::from(frame));
    let mut px: Vec<f32> = (0..n)
        .map(|_| (0.2 + (rng.next_f32() - 0.5) * 0.02) * gain + offset)
        .collect();
    for _ in 0..(n / 500) {
        px[(rng.next_f64() * n as f64) as usize] = 0.95;
    }
    LinearImage::from_planar_channels(ImageDimensions::new(size, 1), [px])
}

const SIZE: Size2us = Size2us::new(1024, 1024);
const FRAMES: u32 = 30;

/// Every combine configuration on one 30-frame set, plus the clone each iteration pays:
/// `stack_images` consumes its frames, so every iteration hands it a fresh copy of a set built
/// once, and the `frame-clone` row is what that copy costs.
#[quick_bench(warmup_time_ms = 200, bench_time_ms = 1000)]
fn bench_stack_30(b: ::quickbench::Bencher) {
    let frames: Vec<StackFrame> = (0..FRAMES).map(|f| synth_frame(SIZE, f).into()).collect();
    b.bench_labeled("frame-clone", || black_box(frames.clone()));
    // σ-clipped mean (2.5σ) with noise weighting and global normalization, the science default;
    // median, the rejection-free baseline; winsorized σ-clip, the dark and bias masters' rejection.
    for (label, config) in [
        ("light", StackConfig::light()),
        ("median", StackConfig::median()),
        ("winsorized", StackConfig::winsorized(3.0)),
    ] {
        b.bench_labeled(label, || {
            black_box(
                stack_images(
                    frames.clone(),
                    &config.clone(),
                    ProgressCallback::default(),
                    CancelToken::never(),
                )
                .unwrap(),
            )
        });
    }
}
