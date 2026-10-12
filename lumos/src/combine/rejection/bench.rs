//! The combine of one block of pixels, a pixel's samples at a time, as each of the combine's tasks
//! reduces its share of a chunk: under every rejection the configuration offers, over the 30
//! frames of the light preset's typical stack and the 300 of a long one. Each pixel's samples are
//! gathered into the buffer the combine reuses, then rejected and averaged, with the quality the
//! standard planes read.

use std::hint::black_box;

use criterion::{BenchmarkId, Criterion, Throughput};

use crate::bench::fixture::{self, Scale};
use crate::bench::scene::{Field, STACKED_LIGHTS};
use crate::combine::frame_cache::sample::PixelSamples;
use crate::combine::frame_cache::sample_noise::NoiseColumns;
use crate::combine::rejection::Rejection;
use crate::combine::rejection::linear_fit_clip_config::LinearFitClipConfig;
use crate::combine::rejection::scratch_buffers::ScratchBuffers;
use crate::combine::rejection::trim_config::TrimConfig;
use crate::combine::stack_config::DEFAULT_MIN_SURVIVORS;
use crate::internals::test_rng::TestRng;
use crate::math::size2us::Size2us;

/// The pixels of a block: one 64 × 64 tile.
pub(crate) const BLOCK: Size2us = Size2us::new(64, 64);

/// A full well's electrons, in which the samples' shot noise is counted.
const FULL_WELL_E: f32 = 50_000.0;

/// The read noise, in electrons.
const READ_NOISE_E: f32 = 3.0;

/// Every rejection the configuration offers, at the settings a user starts from, and its name.
pub(crate) fn rejections() -> [(&'static str, Rejection); 6] {
    [
        ("none", Rejection::None),
        ("sigma_clip", Rejection::sigma_clip(2.5)),
        ("winsorized", Rejection::winsorized(3.0)),
        (
            "linear_fit",
            Rejection::LinearFit(LinearFitClipConfig::default()),
        ),
        ("trim", Rejection::Trim(TrimConfig::default())),
        ("gesd", Rejection::gesd()),
    ]
}

pub(crate) fn benches(c: &mut Criterion) {
    let mut group = fixture::group(c, "kernel/combine/rejection", Scale::Sample);
    for frames in [STACKED_LIGHTS, 300] {
        group.throughput(Throughput::Elements((BLOCK.pixel_count() * frames) as u64));
        for (name, rejection) in rejections() {
            group.bench_function(BenchmarkId::new(name, frames), |b| {
                let block = Block::of(frames);
                let mut scratch = ScratchBuffers::default();
                scratch.reserve(frames);
                let mut values = vec![0.0; frames];
                b.iter(|| {
                    for pixel in block.samples.chunks_exact(frames) {
                        values.copy_from_slice(pixel);
                        let samples = PixelSamples {
                            values: &mut values,
                            weights: &block.weights,
                            noise: Some(block.noise()),
                        };
                        black_box(rejection.combine_mean(
                            samples,
                            DEFAULT_MIN_SURVIVORS,
                            &mut scratch,
                            true,
                        ));
                    }
                });
            });
        }
    }
}

/// A block's samples across a stack's frames, normalized, as the combine gathers them: each
/// pixel's level from a tile of a typical field that holds a star, each frame's sample of it off
/// by that frame's noise, and one sample in a hundred hit by a cosmic ray or a satellite. The
/// frames' sky variance differs by up to 10% either way, and their weights are its inverse, as
/// noise weighting gives them.
#[derive(Debug)]
pub(crate) struct Block {
    /// Pixel by pixel, each pixel's samples frame by frame.
    pub(crate) samples: Vec<f32>,
    pub(crate) weights: Vec<f32>,
    background: Vec<f32>,
    sky: Vec<f32>,
    inverse_electrons: Vec<f32>,
}

impl Block {
    pub(crate) fn of(frames: usize) -> Self {
        let tile = Field::Typical.tile(BLOCK.width);
        let read = READ_NOISE_E / FULL_WELL_E;
        let sky = 0.1;
        let background: Vec<f32> = (0..frames)
            .map(|frame| (read * read + sky / FULL_WELL_E) * (0.9 + 0.05 * (frame % 5) as f32))
            .collect();
        let mut rng = TestRng::new(0xB10C);
        let mut samples = Vec::with_capacity(BLOCK.pixel_count() * frames);
        for &level in tile.pixels() {
            samples.extend(background.iter().map(|&variance| {
                let sigma = (variance + (level - sky).max(0.0) / FULL_WELL_E).sqrt();
                let hit = if rng.next_f32() < 0.01 {
                    0.3 + 0.7 * rng.next_f32()
                } else {
                    0.0
                };
                level + sigma * rng.next_gaussian_f32() + hit
            }));
        }
        Self {
            samples,
            weights: background.iter().map(|variance| 1.0 / variance).collect(),
            sky: vec![sky; frames],
            inverse_electrons: vec![1.0 / FULL_WELL_E; frames],
            background,
        }
    }

    pub(crate) fn noise(&self) -> NoiseColumns<'_> {
        NoiseColumns {
            background: &self.background,
            sky: &self.sky,
            inverse_electrons: &self.inverse_electrons,
        }
    }
}
