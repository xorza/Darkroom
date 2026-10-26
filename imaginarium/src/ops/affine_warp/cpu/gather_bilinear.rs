//! [`GatherBilinear`]: the bilinear warp of one output row of RGB or RGBA pixels, eight pixels a
//! vector, one channel a vector: each lane finds its position and taps as the scalar reference
//! does, and gathers them.

use common::Size2us;
use simd::{F32_LANES, F32x8, I32x8, Isa, Kernel, Mask, Mask8, U8_LANES};

use crate::color::sample::Sample;
use crate::interleaved::row_groups;
use crate::ops::affine_warp::cpu::packed_source::PackedSource;
use crate::ops::affine_warp::cpu::source_map::SourceMap;

/// Output row `y` of a warp of `N`-channel pixels, sampled bilinearly from `source`.
#[derive(Debug)]
pub(super) struct GatherBilinear<'a, T, const N: usize> {
    pub(super) source: PackedSource<'a, T>,
    pub(super) map: SourceMap,
    pub(super) y: usize,
    pub(super) row: &'a mut [T],
}

/// Thirty-two RGBA pixels a step, eight a vector, into the four channels' interleaved store.
impl Kernel for GatherBilinear<'_, u8, 4> {
    type Output = ();

    #[inline(always)]
    fn run<S: Isa>(self, isa: S) {
        let Self {
            source,
            map,
            y,
            row,
        } = self;
        let row_map = RowMap::new(isa, source.size, map, y);
        row_groups::fill::<u8, { 4 * U8_LANES }>(row, |group, out| {
            let pixels = source.pixels;
            let b0 = block_u8::<S, 4>(isa, pixels, row_map, group, 0);
            let b1 = block_u8::<S, 4>(isa, pixels, row_map, group, 1);
            let b2 = block_u8::<S, 4>(isa, pixels, row_map, group, 2);
            let b3 = block_u8::<S, 4>(isa, pixels, row_map, group, 3);
            let channels = [
                isa.narrow_i32_to_u8([b0[0], b1[0], b2[0], b3[0]]),
                isa.narrow_i32_to_u8([b0[1], b1[1], b2[1], b3[1]]),
                isa.narrow_i32_to_u8([b0[2], b1[2], b2[2], b3[2]]),
                isa.narrow_i32_to_u8([b0[3], b1[3], b2[3], b3[3]]),
            ];
            isa.store_interleaved4_u8(channels, out);
        });
    }
}

/// Thirty-two RGB pixels a step, eight a vector, into the three channels' interleaved store.
impl Kernel for GatherBilinear<'_, u8, 3> {
    type Output = ();

    #[inline(always)]
    fn run<S: Isa>(self, isa: S) {
        let Self {
            source,
            map,
            y,
            row,
        } = self;
        let row_map = RowMap::new(isa, source.size, map, y);
        row_groups::fill::<u8, { 3 * U8_LANES }>(row, |group, out| {
            let pixels = source.pixels;
            let b0 = block_u8::<S, 3>(isa, pixels, row_map, group, 0);
            let b1 = block_u8::<S, 3>(isa, pixels, row_map, group, 1);
            let b2 = block_u8::<S, 3>(isa, pixels, row_map, group, 2);
            let b3 = block_u8::<S, 3>(isa, pixels, row_map, group, 3);
            let channels = [
                isa.narrow_i32_to_u8([b0[0], b1[0], b2[0], b3[0]]),
                isa.narrow_i32_to_u8([b0[1], b1[1], b2[1], b3[1]]),
                isa.narrow_i32_to_u8([b0[2], b1[2], b2[2], b3[2]]),
            ];
            isa.store_interleaved3_u8(channels, out);
        });
    }
}

/// Block `block` of eight pixels of group `group` of a `u8` row, its `N` channels rounded.
#[inline(always)]
fn block_u8<S: Isa, const N: usize>(
    isa: S,
    pixels: &[u8],
    row_map: RowMap<S>,
    group: usize,
    block: usize,
) -> [S::I32; N] {
    let full_scale = isa.splat_f32(u8::FULL_SCALE_F32);
    let taps = row_map.taps(isa, group * U8_LANES + block * F32_LANES);
    let mixed = u8::mixed::<S, N>(isa, pixels, taps);
    let mut ints = [isa.splat_i32(0); N];
    for (ints, mixed) in ints.iter_mut().zip(mixed) {
        *ints = isa.round_clamped_f32(mixed, full_scale);
    }
    ints
}

/// Byte `k` of each lane of `word`, a pixel's channel `k`.
#[inline(always)]
fn byte_of<V: I32x8>(word: V, k: usize, byte: V) -> V {
    match k {
        0 => word & byte,
        1 => word.shift_right::<8>() & byte,
        2 => word.shift_right::<16>() & byte,
        _ => word.shift_right::<24>(),
    }
}

/// Eight pixels a step of `u16` and `f32` rows, `G` samples, `N` to a pixel.
macro_rules! eight_pixel_kernel {
    ($sample:ty, $channels:literal) => {
        impl Kernel for GatherBilinear<'_, $sample, $channels> {
            type Output = ();

            #[inline(always)]
            fn run<S: Isa>(self, isa: S) {
                eight_pixels::<S, $sample, $channels, { $channels * F32_LANES }>(isa, self);
            }
        }
    };
}

eight_pixel_kernel!(u16, 3);
eight_pixel_kernel!(u16, 4);
eight_pixel_kernel!(f32, 3);
eight_pixel_kernel!(f32, 4);

/// [`GatherBilinear`] eight pixels, `G` samples, a step, the channels narrowed and stored
/// interleaved.
#[inline(always)]
fn eight_pixels<S: Isa, T: EightPixels, const N: usize, const G: usize>(
    isa: S,
    kernel: GatherBilinear<'_, T, N>,
) {
    debug_assert_eq!(G, N * F32_LANES, "a group is eight pixels");
    let GatherBilinear {
        source,
        map,
        y,
        row,
    } = kernel;
    let row_map = RowMap::new(isa, source.size, map, y);
    row_groups::fill::<T, G>(row, |group, out| {
        let taps = row_map.taps(isa, group * F32_LANES);
        T::store::<S, N, G>(isa, T::mixed::<S, N>(isa, source.pixels, taps), out);
    });
}

/// A sample type whose taps a gather reads.
///
/// Each implementation gathers its taps one by one, in place, and mixes a channel at a time: a
/// closure that gathers is left out of line, as a closure cannot carry `#[inline(always)]`, and
/// an array of the taps' channels goes through the stack on NEON, whose vector is two
/// registers; each measured several times slower there.
trait Gathered: Sample {
    /// The `N` channels of eight pixels, each its four taps mixed.
    fn mixed<S: Isa, const N: usize>(isa: S, pixels: &[Self], taps: Taps<S>) -> [S::F32; N];
}

/// A sample type whose eight-pixel rows [`eight_pixels`] writes.
trait EightPixels: Gathered + Default {
    /// Eight pixels' `N` mixed channels narrowed by the reference's rule, interleaved into `out`.
    fn store<S: Isa, const N: usize, const G: usize>(
        isa: S,
        mixed: [S::F32; N],
        out: &mut [Self; G],
    );
}

/// A pixel's channels are the bytes of one four-byte gather from its first: past an RGB pixel's
/// third, the next pixel's first, or nothing at the end of the image.
impl Gathered for u8 {
    #[inline(always)]
    fn mixed<S: Isa, const N: usize>(isa: S, pixels: &[Self], taps: Taps<S>) -> [S::F32; N] {
        let (stride, byte) = (isa.splat_i32(N as i32), isa.splat_i32(0xFF));
        let w00 = isa.gather_u32_le(pixels, taps.idx[0] * stride, taps.valid[0]);
        let w10 = isa.gather_u32_le(pixels, taps.idx[1] * stride, taps.valid[1]);
        let w01 = isa.gather_u32_le(pixels, taps.idx[2] * stride, taps.valid[2]);
        let w11 = isa.gather_u32_le(pixels, taps.idx[3] * stride, taps.valid[3]);
        let mut mixed = [isa.splat_f32(0.0); N];
        for (k, mixed) in mixed.iter_mut().enumerate() {
            let values = [w00, w10, w01, w11].map(|word| byte_of(word, k, byte).to_f32());
            *mixed = taps.mix(values);
        }
        mixed
    }
}

/// A row's two taps are neighbours: one pair gather reads both pixels' words, transposed.
impl Gathered for u16 {
    #[inline(always)]
    fn mixed<S: Isa, const N: usize>(isa: S, pixels: &[Self], taps: Taps<S>) -> [S::F32; N] {
        let stride = isa.splat_i32(N as i32);
        let top =
            isa.gather_pair_u16::<N>(pixels, taps.idx[0] * stride, taps.valid[0], taps.valid[1]);
        let bottom =
            isa.gather_pair_u16::<N>(pixels, taps.idx[2] * stride, taps.valid[2], taps.valid[3]);
        let mut mixed = [isa.splat_f32(0.0); N];
        for (k, mixed) in mixed.iter_mut().enumerate() {
            *mixed = taps.mix([top[k], top[N + k], bottom[k], bottom[N + k]].map(I32x8::to_f32));
        }
        mixed
    }
}

/// The clamped round leaves each lane in `[0, 65535]`, as the word store takes it.
impl EightPixels for u16 {
    #[inline(always)]
    fn store<S: Isa, const N: usize, const G: usize>(
        isa: S,
        mixed: [S::F32; N],
        out: &mut [Self; G],
    ) {
        let full_scale = isa.splat_f32(Self::FULL_SCALE_F32);
        let mut ints = [isa.splat_i32(0); N];
        for (ints, mixed) in ints.iter_mut().zip(mixed) {
            *ints = isa.round_clamped_f32(mixed, full_scale);
        }
        isa.store_interleaved_u16::<N, G>(ints, out);
    }
}

/// A row's two taps are neighbours: one pair gather reads both pixels' samples, transposed.
impl Gathered for f32 {
    #[inline(always)]
    fn mixed<S: Isa, const N: usize>(isa: S, pixels: &[Self], taps: Taps<S>) -> [S::F32; N] {
        let stride = isa.splat_i32(N as i32);
        let top =
            isa.gather_pair_f32::<N>(pixels, taps.idx[0] * stride, taps.valid[0], taps.valid[1]);
        let bottom =
            isa.gather_pair_f32::<N>(pixels, taps.idx[2] * stride, taps.valid[2], taps.valid[3]);
        let mut mixed = [isa.splat_f32(0.0); N];
        for (k, mixed) in mixed.iter_mut().enumerate() {
            *mixed = taps.mix([top[k], top[N + k], bottom[k], bottom[N + k]]);
        }
        mixed
    }
}

/// `f32` storage keeps the mixed value unclamped.
impl EightPixels for f32 {
    #[inline(always)]
    fn store<S: Isa, const N: usize, const G: usize>(
        isa: S,
        mixed: [S::F32; N],
        out: &mut [Self; G],
    ) {
        isa.store_interleaved_f32::<N, G>(mixed, out);
    }
}

/// One output row's constants for [`Taps`]: the map's terms that hold across the row, splatted,
/// and the image's bounds.
#[derive(Debug, Clone, Copy)]
struct RowMap<S: Isa> {
    across_x: S::F32,
    across_y: S::F32,
    /// `c · py` and `d · py`, the row's own products.
    row_x: S::F32,
    row_y: S::F32,
    translation_x: S::F32,
    translation_y: S::F32,
    half: S::F32,
    /// `k + ½` in lane `k`: each lane's centre past the block's first pixel.
    centres: S::F32,
    one: S::F32,
    minus_one: S::F32,
    minus_two: S::F32,
    width: S::F32,
    width_less_one: S::F32,
    height: S::F32,
    height_less_one: S::F32,
    width_i32: S::I32,
    one_i32: S::I32,
}

impl<S: Isa> RowMap<S> {
    #[inline(always)]
    fn new(isa: S, size: Size2us, map: SourceMap, y: usize) -> Self {
        let py = y as f32 + 0.5;
        let (width, height) = (size.width as f32, size.height as f32);
        Self {
            across_x: isa.splat_f32(map.x_axis.x),
            across_y: isa.splat_f32(map.x_axis.y),
            row_x: isa.splat_f32(map.y_axis.x * py),
            row_y: isa.splat_f32(map.y_axis.y * py),
            translation_x: isa.splat_f32(map.translation.x),
            translation_y: isa.splat_f32(map.translation.y),
            half: isa.splat_f32(0.5),
            centres: isa.load_f32(&[0.5, 1.5, 2.5, 3.5, 4.5, 5.5, 6.5, 7.5]),
            one: isa.splat_f32(1.0),
            minus_one: isa.splat_f32(-1.0),
            minus_two: isa.splat_f32(-2.0),
            width: isa.splat_f32(width),
            width_less_one: isa.splat_f32(width - 1.0),
            height: isa.splat_f32(height),
            height_less_one: isa.splat_f32(height - 1.0),
            width_i32: isa.splat_i32(i32::try_from(size.width).unwrap()),
            one_i32: isa.splat_i32(1),
        }
    }

    /// The taps of output pixels `x` to `x + 7` of the row.
    ///
    /// A tap is in the image where its integer position is, compared as the floored float: a NaN
    /// or a position past `i32` lies outside, as it does for the reference, whose cast saturates
    /// it, or for a NaN, whose weights then make the pixel NaN either way. The index comes from
    /// the floor clamped to one pixel around the image, so the cast is exact, and it matters only
    /// where the tap is in the image.
    #[inline(always)]
    fn taps(self, isa: S, x: usize) -> Taps<S> {
        let px = isa.splat_f32(x as f32) + self.centres;
        let sx = self.across_x * px + self.row_x + self.translation_x - self.half;
        let sy = self.across_y * px + self.row_y + self.translation_y - self.half;
        let (floor_x, floor_y) = (sx.floor(), sy.floor());
        let x0 = floor_x
            .lanes_gt(self.minus_one)
            .and(floor_x.lanes_lt(self.width));
        let x1 = floor_x
            .lanes_gt(self.minus_two)
            .and(floor_x.lanes_lt(self.width_less_one));
        let y0 = floor_y
            .lanes_gt(self.minus_one)
            .and(floor_y.lanes_lt(self.height));
        let y1 = floor_y
            .lanes_gt(self.minus_two)
            .and(floor_y.lanes_lt(self.height_less_one));
        let column = floor_x.max(self.minus_one).min(self.width).to_i32();
        let row = floor_y.max(self.minus_one).min(self.height).to_i32();
        let idx00 = row * self.width_i32 + column;
        let idx01 = idx00 + self.width_i32;
        Taps {
            idx: [idx00, idx00 + self.one_i32, idx01, idx01 + self.one_i32],
            valid: [x0.and(y0), x1.and(y0), x0.and(y1), x1.and(y1)],
            fx: sx - floor_x,
            fy: sy - floor_y,
            one: self.one,
        }
    }
}

/// The bilinear taps of eight output pixels: `(x₀, y₀)`, `(x₀ + 1, y₀)`, `(x₀, y₀ + 1)`,
/// `(x₀ + 1, y₀ + 1)`, each a pixel index and whether it lies in the image, and the fractions.
#[derive(Debug, Clone, Copy)]
struct Taps<S: Isa> {
    idx: [S::I32; 4],
    valid: [Mask<S>; 4],
    fx: S::F32,
    fy: S::F32,
    one: S::F32,
}

impl<S: Isa> Taps<S> {
    /// The bilinear mix of one channel's four tap values: across, then down, each
    /// `a · (1 − t) + b · t` as the scalar reference's `mix`.
    #[inline(always)]
    fn mix(self, values: [S::F32; 4]) -> S::F32 {
        let [c00, c10, c01, c11] = values;
        let (rest_x, rest_y) = (self.one - self.fx, self.one - self.fy);
        let top = c00 * rest_x + c10 * self.fx;
        let bottom = c01 * rest_x + c11 * self.fx;
        top * rest_y + bottom * self.fy
    }
}
