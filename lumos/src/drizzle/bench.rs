//! A set of 30 dithered lights drizzled into the stacked product, as `drizzle_images` drizzles a
//! set held in memory, under the default configuration: a square kernel at pixfrac 0.8 onto twice
//! the frames' side, and the light preset's combine. The lights are 2048 × 2048, so the output
//! fits in memory beside them.

use std::cell::OnceCell;

use common::CancelToken;
use criterion::{BatchSize, BenchmarkId, Criterion, Throughput};

use crate::bench::fixture::{self, Scale};
use crate::bench::scene::{self, Field, STACKED_LIGHTS};
use crate::combine::config::StackConfig;
use crate::drizzle::accumulator::drizzle_frame::DrizzleFrame;
use crate::drizzle::config::DrizzleConfig;
use crate::drizzle::stack::drizzle_images;
use crate::io::image::linear::LinearImage;
use crate::math::size2us::Size2us;
use crate::progress::progress_callback::ProgressCallback;
use crate::registration::transform::WarpTransform;

/// Each light's side.
const SIDE: Size2us = Size2us::new(2048, 2048);

pub(crate) fn benches(c: &mut Criterion) {
    let field = Field::Typical;
    let frames = OnceCell::new();
    let mut group = fixture::group(c, "stage/drizzle", Scale::Frame);
    group.throughput(Throughput::Elements(
        (STACKED_LIGHTS * SIDE.pixel_count()) as u64,
    ));
    let set = format!("{STACKED_LIGHTS}x{}", field.name());
    group.bench_function(BenchmarkId::new("default", set), |b| {
        let frames: &Vec<DrizzleFrame<LinearImage>> = frames.get_or_init(|| {
            (0..STACKED_LIGHTS)
                .map(|index| {
                    let warp = WarpTransform::new(scene::dither(SIDE, index));
                    DrizzleFrame::new(field.exposure(SIDE, index).image, warp)
                })
                .collect()
        });
        b.iter_batched(
            || frames.clone(),
            |frames| {
                drizzle_images(
                    frames,
                    &DrizzleConfig::default(),
                    &StackConfig::light(),
                    ProgressCallback::default(),
                    CancelToken::never(),
                )
                .unwrap()
            },
            BatchSize::PerIteration,
        );
    });
}
