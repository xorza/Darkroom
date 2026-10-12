//! A set of 30 dithered lights drizzled into the stacked product, as `drizzle_images` drizzles a
//! set held in memory, under the default configuration: a square kernel at pixfrac 0.8 onto twice
//! the frames' side, and the light preset's combine. The lights are 2048 × 2048, so the output
//! fits in memory beside them. And a session's calibrated mosaics drizzled from their files, end
//! to end.

use std::cell::OnceCell;
use std::time::Duration;

use common::CancelToken;
use criterion::{BatchSize, BenchmarkId, Criterion, Throughput};

use crate::bench::files::{Files, LIGHTS};
use crate::bench::fixture::{self, Scale};
use crate::bench::scene::{self, FRAME, Field, STACKED_LIGHTS};
use crate::drizzle::drizzle_config::DrizzleConfig;
use crate::drizzle::drizzle_frame::DrizzleFrame;
use crate::image::cfa::cfa_pattern::CfaPattern;
use crate::image::cfa::cfa_type::CfaType;
use crate::image::linear_image::LinearImage;
use crate::ingest::ingest_config::IngestConfig;
use crate::math::size2us::Size2us;
use crate::progress::progress_callback::ProgressCallback;
use crate::registration::transform::warp_transform::WarpTransform;

/// Each light's side.
const SIDE: Size2us = Size2us::new(2048, 2048);

pub(crate) fn benches(c: &mut Criterion) {
    let field = Field::Typical;
    let frames = OnceCell::new();
    // The bench's median iteration on a Ryzen 7 6800U, x86-64-v3.
    let iteration = Duration::from_millis(7200);
    let mut group = fixture::group(c, "stage/drizzle", Scale::Set { iteration });
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
                DrizzleConfig::default()
                    .drizzle_images(
                        frames,
                        &IngestConfig::default(),
                        ProgressCallback::default(),
                        CancelToken::never(),
                    )
                    .unwrap()
            },
            BatchSize::PerIteration,
        );
    });
}

/// `DrizzleConfig::drizzle_cfa_paths` over a session's calibrated lights, mosaics as calibration
/// saves them, end to end under the default configuration and the light preset: each light read,
/// its photosites dropped onto the channels of their colours at twice the frame's side through its
/// dither, and the drops combined with the light preset's noise weighting.
pub(crate) fn pipeline(c: &mut Criterion) {
    let field = Field::Typical;
    let cfa_type = CfaType::Bayer(CfaPattern::Rggb);
    // The bench's median iteration on a Ryzen 7 6800U, x86-64-v3.
    let iteration = Duration::from_millis(64_730);
    let mut group = fixture::group(c, "pipeline/drizzle", Scale::Set { iteration });
    group.throughput(Throughput::Elements((LIGHTS * FRAME.pixel_count()) as u64));
    let set = format!("{LIGHTS}x{}", field.name());
    group.bench_function(BenchmarkId::new("rggb", set), |b| {
        let files = Files::calibrated_mosaics(field, cfa_type);
        let config = DrizzleConfig::default();
        let ingest = IngestConfig::with_cache_dir(files.dir().join("cache"));
        b.iter_batched(
            || {
                files
                    .paths
                    .iter()
                    .enumerate()
                    .map(|(index, path)| {
                        DrizzleFrame::new(path, WarpTransform::new(scene::dither(FRAME, index)))
                    })
                    .collect()
            },
            |frames| {
                config
                    .drizzle_cfa_paths(
                        frames,
                        &ingest,
                        ProgressCallback::default(),
                        CancelToken::never(),
                    )
                    .unwrap()
            },
            BatchSize::PerIteration,
        );
    });
}
