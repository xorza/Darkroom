//! [`Elements`]: a change of sample type at an unchanged channel count, as one vector kernel per
//! pair of sample types.
//!
//! Every kernel is bit-identical to [`Sample::convert`], the scalar reference, a row's last partial
//! group included, which runs through the same step. That is why the widening kernels take the
//! quotient by the source type's full scale as [`F32x8::div_scale`], as the divide rounds it:
//! the reciprocal's product alone, `x * (1.0 / 255.0)`, is doubly rounded and disagrees with
//! `x / 255.0` for 126 of the 256 byte values, and `x * (1.0 / 65535.0)` for 512 of the 65 536
//! word values. For the same reason the float narrowing kernels multiply in `f64`, as the
//! reference does: in `f32`, `x · 255` rounds onto `k + ½` for 128 inputs on `[0, 1]` whose true
//! product lies just past the tie, and `x · 65535` for 32 640. The `u16 → u8` kernel computes
//! `round(v / 257)` exactly in integers.

use simd::{
    F32_LANES, F32x8, Halves, I32_LANES, I32x8, Isa, IsaTier, Kernel, U8_LANES, U8x32, U16_LANES,
    U16x16,
};

use crate::color::color_format::{ColorFormat, SampleType};
use crate::color::sample::Sample;
use crate::interleaved::row_groups::GroupStep;

/// One packed row seen as the element types a kernel converts between: `dst` holds the row's
/// elements, and `src` the same count from the row's start.
#[derive(Debug)]
struct Elements<'a, S, D> {
    src: &'a [S],
    dst: &'a mut [D],
}

impl<'a, S: Sample, D: Sample> Elements<'a, S, D> {
    /// The row `dst` holds, of `dst.len() / size_of::<D>()` elements, from `src`, which runs on
    /// to the end of the image.
    fn new(src: &'a [u8], dst: &'a mut [u8]) -> Self {
        let count = dst.len() / size_of::<D>();
        Self {
            src: bytemuck::cast_slice(&src[..count * size_of::<S>()]),
            dst: bytemuck::cast_slice_mut(dst),
        }
    }
}

/// A pair of sample types an [`Elements`] kernel converts between, picked once per image.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ElementPair {
    U8ToU16,
    U8ToF32,
    U16ToU8,
    U16ToF32,
    F32ToU8,
    F32ToU16,
}

impl ElementPair {
    /// The pair a conversion from `from` to `to` is, or `None` when it changes the channel count
    /// or nothing.
    pub(crate) fn of(from: ColorFormat, to: ColorFormat) -> Option<Self> {
        if from.channel_count != to.channel_count {
            return None;
        }
        Some(match (from.sample_type, to.sample_type) {
            (SampleType::U8, SampleType::U16) => Self::U8ToU16,
            (SampleType::U8, SampleType::F32) => Self::U8ToF32,
            (SampleType::U16, SampleType::U8) => Self::U16ToU8,
            (SampleType::U16, SampleType::F32) => Self::U16ToF32,
            (SampleType::F32, SampleType::U8) => Self::F32ToU8,
            (SampleType::F32, SampleType::U16) => Self::F32ToU16,
            (SampleType::U8, SampleType::U8)
            | (SampleType::U16, SampleType::U16)
            | (SampleType::F32, SampleType::F32) => return None,
        })
    }

    /// One packed row, `src` into `dst`, on `tier`.
    #[inline]
    pub(crate) fn convert_row(self, tier: IsaTier, src: &[u8], dst: &mut [u8]) {
        match self {
            Self::U8ToU16 => tier.run(Elements::<u8, u16>::new(src, dst)),
            Self::U8ToF32 => tier.run(Elements::<u8, f32>::new(src, dst)),
            Self::U16ToU8 => tier.run(Elements::<u16, u8>::new(src, dst)),
            Self::U16ToF32 => tier.run(Elements::<u16, f32>::new(src, dst)),
            Self::F32ToU8 => tier.run(Elements::<f32, u8>::new(src, dst)),
            Self::F32ToU16 => tier.run(Elements::<f32, u16>::new(src, dst)),
        }
    }
}

/// A kernel between two sample types: its row through its [`GroupStep`] of `$lanes` elements.
macro_rules! element_kernel {
    ($from:ty, $to:ty, $lanes:expr) => {
        impl Kernel for Elements<'_, $from, $to> {
            type Output = ();

            #[inline(always)]
            fn run<V: Isa>(self, isa: V) {
                <Self as GroupStep<$from, $to, { $lanes }, { $lanes }>>::run_row(
                    isa, self.src, self.dst,
                );
            }
        }
    };
}

element_kernel!(u8, u16, U8_LANES);
element_kernel!(u8, f32, U8_LANES);
element_kernel!(u16, u8, U8_LANES);
element_kernel!(u16, f32, U16_LANES);
element_kernel!(f32, u8, U8_LANES);
element_kernel!(f32, u16, U16_LANES);

/// `v · 257`, which is `v` in both bytes and exact for every byte.
impl GroupStep<u8, u16, U8_LANES, U8_LANES> for Elements<'_, u8, u16> {
    #[inline(always)]
    fn step<V: Isa>(isa: V, group: &[u8; U8_LANES], out: &mut [u16; U8_LANES]) {
        let factor = isa.splat_u16(257);
        let words = isa.load_u8(group).widen();
        let (out, _) = out.as_chunks_mut::<U16_LANES>();
        words.low.mul_low(factor).store(&mut out[0]);
        words.high.mul_low(factor).store(&mut out[1]);
    }
}

/// `v / 255`, as the divide rounds it.
impl GroupStep<u8, f32, U8_LANES, U8_LANES> for Elements<'_, u8, f32> {
    #[inline(always)]
    fn step<V: Isa>(isa: V, group: &[u8; U8_LANES], out: &mut [f32; U8_LANES]) {
        let (divisor, reciprocal) = (isa.splat_f32(255.0), isa.splat_f32(1.0 / 255.0));
        let (octets, _) = group.as_chunks::<I32_LANES>();
        let (out, _) = out.as_chunks_mut::<F32_LANES>();
        isa.load_u8_as_i32(&octets[0])
            .to_f32()
            .div_scale(divisor, reciprocal)
            .store(&mut out[0]);
        isa.load_u8_as_i32(&octets[1])
            .to_f32()
            .div_scale(divisor, reciprocal)
            .store(&mut out[1]);
        isa.load_u8_as_i32(&octets[2])
            .to_f32()
            .div_scale(divisor, reciprocal)
            .store(&mut out[2]);
        isa.load_u8_as_i32(&octets[3])
            .to_f32()
            .div_scale(divisor, reciprocal)
            .store(&mut out[3]);
    }
}

/// `round(v / 257)`, exactly: `⌊v · 65281 / 2¹⁶⌋` stays within 65280, so adding 128 cannot
/// overflow, and the shift by 8 then gives the nearest quotient for every `v` — `v / 257` is never
/// a tie, 257 being odd.
impl GroupStep<u16, u8, U8_LANES, U8_LANES> for Elements<'_, u16, u8> {
    #[inline(always)]
    fn step<V: Isa>(isa: V, group: &[u16; U8_LANES], out: &mut [u8; U8_LANES]) {
        let (multiplier, half) = (isa.splat_u16(65281), isa.splat_u16(128));
        let (halves, _) = group.as_chunks::<U16_LANES>();
        let low = (isa.load_u16(&halves[0]).mul_high(multiplier) + half).shift_right::<8>();
        let high = (isa.load_u16(&halves[1]).mul_high(multiplier) + half).shift_right::<8>();
        low.narrow_u8(high).store(out);
    }
}

/// `v / 65535`, as the divide rounds it.
impl GroupStep<u16, f32, U16_LANES, U16_LANES> for Elements<'_, u16, f32> {
    #[inline(always)]
    fn step<V: Isa>(isa: V, group: &[u16; U16_LANES], out: &mut [f32; U16_LANES]) {
        let (divisor, reciprocal) = (isa.splat_f32(65535.0), isa.splat_f32(1.0 / 65535.0));
        let ints = isa.load_u16(group).widen();
        let (out, _) = out.as_chunks_mut::<F32_LANES>();
        ints.low
            .to_f32()
            .div_scale(divisor, reciprocal)
            .store(&mut out[0]);
        ints.high
            .to_f32()
            .div_scale(divisor, reciprocal)
            .store(&mut out[1]);
    }
}

/// [`scaled`] at 255, into bytes.
impl GroupStep<f32, u8, U8_LANES, U8_LANES> for Elements<'_, f32, u8> {
    #[inline(always)]
    fn step<V: Isa>(isa: V, group: &[f32; U8_LANES], out: &mut [u8; U8_LANES]) {
        let full_scale = isa.splat_f64(255.0);
        let (octets, _) = group.as_chunks::<F32_LANES>();
        let f64s = [0, 1, 2, 3].map(|i| scaled(isa.load_f32(&octets[i]), full_scale));
        isa.round_clamped_u8(f64s, full_scale).store(out);
    }
}

/// [`scaled`] at 65535, into words.
impl GroupStep<f32, u16, U16_LANES, U16_LANES> for Elements<'_, f32, u16> {
    #[inline(always)]
    fn step<V: Isa>(isa: V, group: &[f32; U16_LANES], out: &mut [u16; U16_LANES]) {
        let full_scale = isa.splat_f64(65535.0);
        let (octets, _) = group.as_chunks::<F32_LANES>();
        let f64s = [0, 1].map(|i| scaled(isa.load_f32(&octets[i]), full_scale));
        isa.round_clamped_u16(f64s, full_scale).store(out);
    }
}

/// Eight samples on `[0, 1]` scaled by `full_scale` in `f64`, for the Isa's clamped round, which
/// is [`Sample::from_unit`]'s: the product clamped to `[0, full_scale]` — a NaN product to zero —
/// and rounded to nearest, ties to even. Clamping before the round is the reference's round
/// before its saturating cast, the bounds being integers.
#[inline(always)]
fn scaled<F: F32x8>(values: F, full_scale: F::F64) -> Halves<F::F64> {
    let halves = values.widen();
    Halves {
        low: halves.low * full_scale,
        high: halves.high * full_scale,
    }
}
