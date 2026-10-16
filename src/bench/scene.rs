//! The standard scene every case measures on: sky, drawn from fixed seeds so two runs time the
//! same work.

use std::f32::consts::TAU;

use rayon::prelude::*;

use crate::common::color_format::{ColorFormat, SampleType};
use crate::common::sample::Sample;
use crate::image::Image;
use crate::image::image_desc::ImageDesc;

/// The standard frame's width: 6000 × 4000, an APS-C camera's 24 MP, as the lumos benches use.
pub(crate) const WIDTH: usize = 6000;
/// The standard frame's height.
pub(crate) const HEIGHT: usize = 4000;
/// The side of the tile an atomic case hands a kernel that takes an image.
pub(crate) const TILE: usize = 512;

/// `width` × `height` pixels of sky in `format` from `seed`: in each color channel 0.1 with
/// white noise of σ 0.01, one pixel in fifty a star up to 1 above it in every color channel, all
/// clamped to `[0, 1]`; alpha opaque. Each pixel draws from its own `split_mix` stream, so rows
/// fill in parallel and a 24 MP frame takes a fraction of a second.
pub(crate) fn sky(format: ColorFormat, width: usize, height: usize, seed: u64) -> Image {
    match format.sample_type {
        SampleType::U8 => typed_sky::<u8>(format, width, height, seed),
        SampleType::U16 => typed_sky::<u16>(format, width, height, seed),
        SampleType::F32 => typed_sky::<f32>(format, width, height, seed),
    }
}

fn typed_sky<T: Sample>(format: ColorFormat, width: usize, height: usize, seed: u64) -> Image {
    let channels = format.channel_count.count();
    let color = channels - usize::from(format.has_alpha());
    let mut samples = vec![T::FULL_SCALE; width * height * channels];
    samples
        .par_chunks_mut(width * channels)
        .enumerate()
        .for_each(|(y, row)| {
            for (x, pixel) in row.chunks_exact_mut(channels).enumerate() {
                let mut draws = SplitMix((seed << 40) ^ (y * width + x) as u64);
                let star = if draws.next().is_multiple_of(50) {
                    draws.unit()
                } else {
                    0.0
                };
                for sample in &mut pixel[..color] {
                    let value = 0.1 + 0.01 * draws.gaussian() + star;
                    *sample = T::from_unit(value.clamp(0.0, 1.0));
                }
            }
        });
    Image::from_samples(ImageDesc::new(width, height, format), samples).unwrap()
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
