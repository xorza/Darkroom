//! [`BlendRow`]: the blend of one row pair, as one vector kernel per sample type.
//!
//! A vector holds the channels as they lie in the row, and every lane takes the mode; in an RGBA
//! row a select then restores `src` in each pixel's alpha lane, which `Normal` blends. Every step
//! is the scalar reference's, operation for operation, so a kernel is bit-identical to it: the
//! integer paths *divide* by the full scale, as [`Sample::to_unit`] does, where multiplying by
//! the reciprocal would be cheaper but disagrees for 126 of the 256 byte values, and the alpha
//! mix rounds its multiplies and its add apart. Refused: [`F32x8::div_scale`], exact and the
//! faster quotient for a conversion, which measured 1.00–1.16× slower here on Zen 3: the blend
//! keeps the float pipes busy, and the divider runs beside them.

use simd::{
    F32_LANES, F32x8, I32_LANES, I32x8, Isa, Kernel, Mask8, U8_LANES, U8x32, U16_LANES, U16x16,
};

use crate::color::sample::Sample;
use crate::interleaved::row_groups;
use crate::ops::blend::{Blend, BlendMode};

/// One row pair of `T` samples blended into `out`, all three one row.
#[derive(Debug)]
pub(super) struct BlendRow<'a, T> {
    pub(super) params: Blend,
    pub(super) alpha: Alpha,
    pub(super) src: &'a [T],
    pub(super) dst: &'a [T],
    pub(super) out: &'a mut [T],
}

/// Whether a row's pixels carry alpha, which the mode leaves to the mix alone.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Alpha {
    /// Mono or RGB: every channel takes the mode.
    Absent,
    /// RGBA: the fourth channel of each pixel blends as `Normal`.
    Mixed,
}

impl<'a, T: Sample> BlendRow<'a, T> {
    /// The rows of an image of `T` samples, as bytes.
    pub(super) fn new(
        params: Blend,
        alpha: Alpha,
        src: &'a [u8],
        dst: &'a [u8],
        out: &'a mut [u8],
    ) -> Self {
        Self {
            params,
            alpha,
            src: bytemuck::cast_slice(src),
            dst: bytemuck::cast_slice(dst),
            out: bytemuck::cast_slice_mut(out),
        }
    }
}

/// Thirty-two bytes a step: four vectors of eight.
impl Kernel for BlendRow<'_, u8> {
    type Output = ();

    #[inline(always)]
    fn run<S: Isa>(self, isa: S) {
        let splat = Splat::new(isa, self.params, self.alpha, u8::FULL_SCALE_F32);
        row_groups::zip::<u8, U8_LANES>(self.src, self.dst, self.out, |src, dst, out| {
            let [src, dst] = [src, dst].map(|bytes| {
                let (octets, _) = bytes.as_chunks::<I32_LANES>();
                [0, 1, 2, 3].map(|idx| isa.load_u8_as_i32(&octets[idx]).to_f32() / splat.full_scale)
            });
            let ints = [0, 1, 2, 3].map(|idx| splat.blend_rounded(isa, src[idx], dst[idx]));
            isa.narrow_i32_to_u8(ints).store(out);
        });
    }
}

/// Sixteen words a step: two vectors of eight.
impl Kernel for BlendRow<'_, u16> {
    type Output = ();

    #[inline(always)]
    fn run<S: Isa>(self, isa: S) {
        let splat = Splat::new(isa, self.params, self.alpha, u16::FULL_SCALE_F32);
        row_groups::zip::<u16, U16_LANES>(self.src, self.dst, self.out, |src, dst, out| {
            let [src, dst] = [src, dst].map(|words| {
                let ints = isa.load_u16(words).widen();
                [ints.low, ints.high].map(|ints| ints.to_f32() / splat.full_scale)
            });
            let [low, high] = [0, 1].map(|idx| splat.blend_rounded(isa, src[idx], dst[idx]));
            low.narrow_u16(high).store(out);
        });
    }
}

/// Eight samples a step. The full scale is one, so the reference's multiply by it is exact and
/// the clamp is the whole narrowing.
impl Kernel for BlendRow<'_, f32> {
    type Output = ();

    #[inline(always)]
    fn run<S: Isa>(self, isa: S) {
        let splat = Splat::new(isa, self.params, self.alpha, f32::FULL_SCALE_F32);
        row_groups::zip::<f32, F32_LANES>(self.src, self.dst, self.out, |src, dst, out| {
            let blended = splat.blend(isa.load_f32(src), isa.load_f32(dst));
            splat
                .zero
                .max(blended)
                .min_positive(splat.full_scale)
                .store(out);
        });
    }
}

/// The blend's constants, splatted once per row, and [`BlendMode::blend`] on them.
#[derive(Debug, Clone, Copy)]
struct Splat<F: F32x8> {
    mode: BlendMode,
    alpha: F,
    one_minus_alpha: F,
    zero: F,
    one: F,
    half: F,
    two: F,
    full_scale: F,
    /// Lanes 3 and 7 in an RGBA row, the alpha channel of each pixel; none in another.
    alpha_lanes: F::Mask,
}

impl<F: F32x8> Splat<F> {
    #[inline(always)]
    fn new<S: Isa<F32 = F>>(isa: S, params: Blend, alpha: Alpha, full_scale: f32) -> Self {
        let zero = isa.splat_f32(0.0);
        let alpha_lanes = match alpha {
            Alpha::Absent => [0.0; F32_LANES],
            Alpha::Mixed => [0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0],
        };
        Self {
            mode: params.mode,
            alpha: isa.splat_f32(params.alpha),
            one_minus_alpha: isa.splat_f32(1.0 - params.alpha),
            zero,
            one: isa.splat_f32(1.0),
            half: isa.splat_f32(0.5),
            two: isa.splat_f32(2.0),
            full_scale: isa.splat_f32(full_scale),
            alpha_lanes: isa.load_f32(&alpha_lanes).lanes_gt(zero),
        }
    }

    /// The blend of an integer row's normalized values, scaled back by the full scale and
    /// narrowed by [`Sample::from_f32`]'s rule, as the clamped round takes it.
    #[inline(always)]
    fn blend_rounded<S: Isa<F32 = F>>(self, isa: S, src: F, dst: F) -> S::I32 {
        isa.round_clamped_f32(self.blend(src, dst) * self.full_scale, self.full_scale)
    }

    /// [`BlendMode::blend`] of normalized channels, `Normal` in the alpha lanes.
    ///
    /// `max` and `min` are compare-selects, `a > b ? a : b` and `a > b ? b : a`, which the
    /// reference spells out, so each takes a NaN or a signed zero to the same side.
    #[inline(always)]
    fn blend(self, src: F, dst: F) -> F {
        let blended = match self.mode {
            BlendMode::Normal => src,
            BlendMode::Add => self.one.min(src + dst),
            BlendMode::Subtract => (dst - src).max(self.zero),
            BlendMode::Multiply => src * dst,
            BlendMode::Screen => self.one - (self.one - src) * (self.one - dst),
            BlendMode::Overlay => {
                let dark = self.two * src * dst;
                let light = self.one - self.two * (self.one - src) * (self.one - dst);
                dst.lanes_lt(self.half).select(dark, light)
            }
        };
        let blended = self.alpha_lanes.select(src, blended);
        blended * self.alpha + dst * self.one_minus_alpha
    }
}
