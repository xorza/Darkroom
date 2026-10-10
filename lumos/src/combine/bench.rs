//! Benchmarks for the in-memory combine engine (`stack_images`), isolated from RAW decode.
//!
//! Builds a synthetic light-frame set in memory once and stacks copies of it, so the measured time
//! is the combine hot path — normalization, weight resolution, per-pixel rejection and weighted
//! accumulation — plus a clone of the set, which a separate row reports.
//!
//! Run: `cargo test -p lumos --release --features bench combine::bench -- --ignored --nocapture`

use crate::internals::prelude::*;
use quickbench::quick_bench;
use std::hint::black_box;

use crate::combine::config::{Combine, CombineMethod, Normalization, StackConfig, Weighting};
use crate::combine::rejection::Rejection;
use crate::combine::stack::{StackFrame, stack_images};
use crate::frame_store::frame_quality::FrameQuality;
use crate::progress::progress_callback::ProgressCallback;

/// A 1 MP mono frame: smooth background + per-frame offset/gain (so normalization has work to do) +
/// ~0.2% bright outliers (so rejection has something to clip).
#[expect(
    clippy::cast_sign_loss,
    reason = "synthetic fixtures are small images with non-negative coordinates"
)]
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
        (
            "median",
            StackConfig {
                combine: Combine::median(),
                weighting: Weighting::Equal,
                normalization: Normalization::None,
                ..StackConfig::light()
            },
        ),
        ("winsorized", StackConfig::bias_or_dark()),
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

const LARGE_SIZE: Size2us = Size2us::new(256, 256);
const LARGE_FRAMES: u32 = 300;

/// The rejections whose per-pixel cost grows with the count, at 300 frames: linear fit, which reads
/// the normal scores of every count a pixel has, and GESD. Each once on a set every frame covers
/// whole, and once on a ragged set, each frame missing a band of columns of its own width, so the
/// count changes along every row the way a registered stack's edges change it.
#[quick_bench(warmup_time_ms = 200, bench_time_ms = 2000)]
fn bench_stack_300(b: ::quickbench::Bencher) {
    let whole: Vec<StackFrame> = (0..LARGE_FRAMES)
        .map(|f| synth_frame(LARGE_SIZE, f).into())
        .collect();
    let ragged: Vec<StackFrame> = whole
        .iter()
        .cloned()
        .enumerate()
        .map(|(f, mut frame)| {
            let missing = f % 97;
            frame.quality = FrameQuality::from_coverage(Buffer2::new(
                LARGE_SIZE.width,
                LARGE_SIZE.height,
                (0..LARGE_SIZE.pixel_count())
                    .map(|i| {
                        if i % LARGE_SIZE.width < missing {
                            0.0
                        } else {
                            1.0
                        }
                    })
                    .collect(),
            ));
            frame
        })
        .collect();
    for (set, frames) in [("whole", &whole), ("ragged", &ragged)] {
        for (label, rejection) in [
            ("linear fit", Rejection::linear_fit(3.0)),
            ("gesd", Rejection::gesd()),
        ] {
            let config = StackConfig {
                combine: Combine {
                    method: CombineMethod::Mean(rejection),
                    ..Combine::mean()
                },
                ..StackConfig::light()
            };
            b.bench_labeled(&format!("{set} {label}"), || {
                black_box(
                    stack_images(
                        frames.clone(),
                        &config,
                        ProgressCallback::default(),
                        CancelToken::never(),
                    )
                    .unwrap(),
                )
            });
        }
    }
}
