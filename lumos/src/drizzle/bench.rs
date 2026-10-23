//! A set of 30 dithered lights drizzled into the stacked product, as `drizzle_images` drizzles a
//! set held in memory, under the default configuration: a square kernel at pixfrac 0.8 onto twice
//! the frames' side, and the light preset's combine. The lights are 2048 × 2048, so the output
//! fits in memory beside them. And a session's calibrated mosaics drizzled from their files, end
//! to end.

use std::cell::OnceCell;
use std::time::Duration;

use criterion::{BatchSize, BenchmarkId, Criterion, Throughput};

use crate::bench::files::{Files, LIGHTS};
use crate::bench::fixture::{Kind, Scale};
use crate::bench::scene::{self, FRAME, Field, STACKED_LIGHTS};
use crate::drizzle::accumulator;
use crate::drizzle::drizzle_config::DrizzleConfig;
use crate::drizzle::drizzle_frame::DrizzleFrame;
use crate::image::cfa::cfa_pattern::CfaPattern;
use crate::image::cfa::cfa_type::CfaType;
use crate::image::cfa::mosaic::Mosaic;
use crate::image::linear_image::LinearImage;
use crate::ingest::ingest_config::IngestConfig;
use crate::math::size2us::Size2us;
use crate::progress::progress_monitor::ProgressMonitor;
use crate::registration::warp_transform::WarpTransform;

/// Each light's side.
const SIDE: Size2us = Size2us::new(2048, 2048);

/// The stage benches, then the entry point end to end.
pub(crate) fn atomic(c: &mut Criterion) {
    accumulator::bench::atomic(c);
}

pub(crate) fn integration(c: &mut Criterion) {
    stage(c);
    end_to_end(c);
}

fn stage(c: &mut Criterion) {
    let field = Field::Typical;
    let frames = OnceCell::new();
    // The bench's median iteration on a Ryzen 7 6800U, x86-64-v3.
    let iteration = Duration::from_millis(7200);
    let mut group = Kind::Integration.group(c, "drizzle", Scale::Set { iteration });
    group.throughput(Throughput::Elements(
        (STACKED_LIGHTS * SIDE.pixel_count()) as u64,
    ));
    let set = format!("{STACKED_LIGHTS}x{}", field.name());
    group.bench_function(BenchmarkId::new("default", set), |b| {
        let frames: &Vec<DrizzleFrame<LinearImage>> = frames.get_or_init(|| {
            (0..STACKED_LIGHTS)
                .map(|idx| {
                    let warp = WarpTransform::new(scene::dither(SIDE, idx));
                    DrizzleFrame::new(field.exposure(SIDE, idx).image, warp)
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
                        ProgressMonitor::unattended(),
                    )
                    .unwrap()
            },
            BatchSize::PerIteration,
        );
    });
}

/// `DrizzleConfig::drizzle_cfa_paths` over a session's calibrated lights, mosaics as calibration
/// saves them, end to end under the default configuration and the light preset: each light read,
/// its photosites dropped onto the channels of their colors at twice the frame's side through its
/// dither, and the drops combined with the light preset's noise weighting.
fn end_to_end(c: &mut Criterion) {
    let field = Field::Typical;
    let cfa_type = CfaType::Mosaic(Mosaic::Bayer(CfaPattern::Rggb));
    // The bench's median iteration on a Ryzen 7 6800U, x86-64-v3.
    let iteration = Duration::from_millis(64_730);
    let mut group = Kind::Integration.group(c, "pipeline/drizzle", Scale::Set { iteration });
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
                    .map(|(idx, path)| {
                        DrizzleFrame::new(path, WarpTransform::new(scene::dither(FRAME, idx)))
                    })
                    .collect()
            },
            |frames| {
                config
                    .drizzle_cfa_paths(frames, &ingest, ProgressMonitor::unattended())
                    .unwrap()
            },
            BatchSize::PerIteration,
        );
    });
}
