//! The standard scene every case measures on: sky, drawn from fixed seeds so two runs time the
//! same work.

use std::f32::consts::TAU;

use common::{USize2, par};
use rayon::prelude::*;

use crate::color::color_format::{ColorFormat, SampleType};
use crate::color::sample::Sample;
use crate::interleaved::interleaved_desc::InterleavedDesc;
use crate::interleaved::interleaved_image::InterleavedImage;

/// The standard frame: 6000 × 4000, an APS-C camera's 24 MP, as the lumos benches use.
pub(crate) const FRAME: USize2 = USize2::new(6000, 4000);
/// The side of the tile an atomic case hands a kernel that takes an image.
pub(crate) const TILE: usize = 512;

/// `size` pixels of sky in `format` from `seed`: in each color channel 0.1 with
/// white noise of σ 0.01, one pixel in fifty a star up to 1 above it in every color channel, all
/// clamped to `[0, 1]`; alpha opaque. Each pixel draws from its own `split_mix` stream, so rows
/// fill in parallel and a 24 MP frame takes a fraction of a second.
pub(crate) fn sky(format: ColorFormat, size: USize2, seed: u64) -> InterleavedImage {
    match format.sample_type {
        SampleType::U8 => typed_sky::<u8>(format, size, seed),
        SampleType::U16 => typed_sky::<u16>(format, size, seed),
        SampleType::F32 => typed_sky::<f32>(format, size, seed),
    }
}

fn typed_sky<T: Sample>(format: ColorFormat, size: USize2, seed: u64) -> InterleavedImage {
    let USize2 { width, height } = size;
    let channel_count = format.channel_count.count();
    let color_count = channel_count - usize::from(format.has_alpha());
    let mut samples = vec![T::FULL_SCALE; width * height * channel_count];
    par::chunks_mut(&mut samples, width * channel_count)
        .enumerate()
        .for_each(|(y, row)| {
            for (x, pixel) in row.chunks_exact_mut(channel_count).enumerate() {
                let mut draws = SplitMix((seed << 40) ^ (y * width + x) as u64);
                let star = if draws.next().is_multiple_of(50) {
                    draws.unit()
                } else {
                    0.0
                };
                for sample in &mut pixel[..color_count] {
                    let value = 0.1 + 0.01 * draws.gaussian() + star;
                    *sample = T::from_unit(value.clamp(0.0, 1.0));
                }
            }
        });
    InterleavedImage::from_samples(InterleavedDesc::new(size, format), samples)
}

/// Steele, Lea and Flood's SplitMix64: one 64-bit state, a fresh output per call.
#[derive(Debug)]
struct SplitMix(u64);

impl SplitMix {
    const fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// A draw on `[0, 1)` from the output's top 24 bits, every one exact in an `f32`.
    fn unit(&mut self) -> f32 {
        (self.next() >> 40) as f32 / (1u32 << 24) as f32
    }

    /// A draw of a unit normal, by Box–Muller.
    fn gaussian(&mut self) -> f32 {
        let (u, v) = (1.0 - self.unit(), self.unit());
        (-2.0 * u.ln()).sqrt() * (TAU * v).cos()
    }
}
