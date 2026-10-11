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
/// the normal scores of every count a pixel has, GESD, and winsorized, whose clamp walks the
/// samples once more for every step it takes. Each once on a set every frame covers
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
            ("winsorized", Rejection::winsorized(2.5)),
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

const SPILLED_FRAMES: u32 = 40;
/// A chunk budget well under the 160 MiB the frames hold, so the walk takes several chunks.
const SPILLED_CHUNK_MEMORY: u64 = 64 << 20;

/// The weighted mean of 40 spilled 1 MP frames on a 64 MiB chunk budget, the reads of the spill
/// tier against the reduction: `warm` with every plane in the page cache, `cold` with every plane
/// evicted before the pass, and `evict` the eviction alone, which `cold` includes. Where the
/// system cannot evict, `cold` reads warm and says nothing of the disk.
#[quick_bench(warmup_time_ms = 500, bench_time_ms = 5000)]
fn bench_spilled_mean_40(b: ::quickbench::Bencher) {
    use common::TempDir;

    use crate::combine::cache::core::CacheCore;
    use crate::combine::cache::sample_noise::SampleNoise;
    use crate::combine::cache::slots::Slots;
    use crate::combine::cache::{CombineRequest, FrameCache};
    use crate::combine::config::DEFAULT_MIN_SURVIVORS;
    use crate::frame_store::frame_stats::FrameStats;
    use crate::frame_store::run_scratch::RunScratch;
    use crate::frame_store::stored_frame::StoredFrame;
    use crate::frame_store::tier::Tier;
    use crate::stack_product::quality_planes::QualityPlanes;

    let directory = TempDir::new("lumos_bench_spilled_mean");
    let scratch = RunScratch::create(directory.path()).unwrap();
    let frames: Vec<StoredFrame> = (0..SPILLED_FRAMES)
        .map(|f| {
            let image = synth_frame(SIZE, f);
            let stats = FrameStats::measure(&image);
            StoredFrame::spill(&scratch, &image, &FrameQuality::None, stats).unwrap()
        })
        .collect();
    let dimensions = ImageDimensions::new(SIZE, 1);
    let cache = FrameCache::from_stored_frames(
        frames,
        CacheCore::plain(
            Tier::Spilled {
                scratch,
                chunk_memory: SPILLED_CHUNK_MEMORY,
                overcommit_bytes: 0,
                beside_bytes: 0,
            },
            dimensions,
        ),
        Normalization::None,
    )
    .unwrap();
    let slots = Slots::new(None, 1);
    let noise = SampleNoise::new(
        cache.frames.iter().map(|frame| &frame.source_stats),
        None,
        slots,
    );
    let mean = || {
        let request = CombineRequest {
            weights: None,
            planes: QualityPlanes::STANDARD,
            min_survivors: DEFAULT_MIN_SURVIVORS,
            noise: Some(&noise),
            slots,
        };
        black_box(cache.process_chunked(request, |samples, scratch| {
            Rejection::sigma_clip(2.5).combine_mean(samples, DEFAULT_MIN_SURVIVORS, scratch, true)
        }))
    };
    let evict = || {
        for frame in &cache.frames {
            for plane in &frame.channels {
                black_box(plane.evict());
            }
        }
    };
    b.bench_labeled("warm", mean);
    b.bench_labeled("evict", evict);
    b.bench_labeled("cold", || {
        evict();
        mean()
    });
}

/// The issue's case of a spilled combine under a memory cgroup: 40 frames of 2048² (640 MiB)
/// combined on a 128 MiB chunk budget, the bytes the process read from storage
/// (`/proc/self/io`'s `read_bytes`) and the time reported. Run alone under a cap, as
/// `systemd-run --user --scope -p MemoryMax=320M <test binary> --exact <this> --ignored
/// --nocapture`: the reads should stay near the spill size, one pass of it.
#[test]
#[ignore = "a manual check under a memory cgroup"]
fn spilled_mean_under_a_memory_cap() {
    use std::fs;
    use std::time::Instant;

    use common::TempDir;

    use crate::combine::cache::core::CacheCore;
    use crate::combine::cache::sample_noise::SampleNoise;
    use crate::combine::cache::slots::Slots;
    use crate::combine::cache::{CombineRequest, FrameCache};
    use crate::combine::config::DEFAULT_MIN_SURVIVORS;
    use crate::frame_store::frame_stats::FrameStats;
    use crate::frame_store::run_scratch::RunScratch;
    use crate::frame_store::stored_frame::StoredFrame;
    use crate::frame_store::tier::Tier;
    use crate::stack_product::quality_planes::QualityPlanes;

    let read_bytes = || -> u64 {
        fs::read_to_string("/proc/self/io")
            .unwrap()
            .lines()
            .find_map(|line| line.strip_prefix("read_bytes: "))
            .unwrap()
            .parse()
            .unwrap()
    };
    let size = Size2us::new(2048, 2048);
    let directory = TempDir::new("lumos_spilled_mean_cap");
    let scratch = RunScratch::create(directory.path()).unwrap();
    let frames: Vec<StoredFrame> = (0..40)
        .map(|f| {
            let image = synth_frame(size, f);
            let stats = FrameStats::measure(&image);
            StoredFrame::spill(&scratch, &image, &FrameQuality::None, stats).unwrap()
        })
        .collect();
    for frame in &frames {
        for plane in &frame.channels {
            plane.evict();
        }
    }
    let cache = FrameCache::from_stored_frames(
        frames,
        CacheCore::plain(
            Tier::Spilled {
                scratch,
                chunk_memory: 128 << 20,
                overcommit_bytes: 0,
                beside_bytes: 0,
            },
            ImageDimensions::new(size, 1),
        ),
        Normalization::None,
    )
    .unwrap();
    let slots = Slots::new(None, 1);
    let noise = SampleNoise::new(
        cache.frames.iter().map(|frame| &frame.source_stats),
        None,
        slots,
    );
    let before = read_bytes();
    let started = Instant::now();
    let request = CombineRequest {
        weights: None,
        planes: QualityPlanes::STANDARD,
        min_survivors: DEFAULT_MIN_SURVIVORS,
        noise: Some(&noise),
        slots,
    };
    black_box(cache.process_chunked(request, |samples, scratch| {
        Rejection::sigma_clip(2.5).combine_mean(samples, DEFAULT_MIN_SURVIVORS, scratch, true)
    }));
    eprintln!(
        "read {} MiB in {:?} for a 640 MiB spill",
        (read_bytes() - before) >> 20,
        started.elapsed()
    );
}
