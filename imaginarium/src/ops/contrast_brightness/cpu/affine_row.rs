//! [`AffineRow`]: [`ChannelAffine`] over one row in place, as one vector kernel per sample type.
//!
//! A vector holds the channels as they lie in the row, and every lane takes an affine: the
//! channel's, or in an RGBA row's alpha lanes one that keeps the value. The multiply and the add
//! round apart, as the reference's do, so every kernel is bit-identical to it.

use simd::{F32_LANES, F32x8, I32_LANES, I32x8, Isa, Kernel, U8_LANES, U8x32, U16_LANES, U16x16};

use crate::color::sample::Sample;
use crate::interleaved::row_groups;
use crate::ops::contrast_brightness::channel_affine::ChannelAffine;

/// One row of `T` samples adjusted in place.
#[derive(Debug)]
pub(super) struct AffineRow<'a, T> {
    pub(super) affine: ChannelAffine,
    pub(super) alpha: Alpha,
    pub(super) row: &'a mut [T],
}

/// What an [`AffineRow`] does to the fourth channel of each pixel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Alpha {
    /// No pixel has an alpha channel: every channel is a color.
    Absent,
    /// The row is RGBA, and each pixel's alpha stays as it is.
    Kept,
}

impl<'a, T: Sample> AffineRow<'a, T> {
    /// A row of an image of `T` samples, as bytes.
    pub(super) fn new(affine: ChannelAffine, alpha: Alpha, row: &'a mut [u8]) -> Self {
        Self {
            affine,
            alpha,
            row: bytemuck::cast_slice_mut(row),
        }
    }
}

/// Thirty-two bytes a step, through four vectors of `f32`.
impl Kernel for AffineRow<'_, u8> {
    type Output = ();

    #[inline(always)]
    fn run<S: Isa>(self, isa: S) {
        let lanes = Lanes::new(isa, self.affine, self.alpha);
        row_groups::in_place::<u8, U8_LANES>(self.row, |bytes| {
            let (octets, _) = bytes.as_chunks::<I32_LANES>();
            let ints = [
                lanes.rounded(isa, isa.load_u8_as_i32(&octets[0]).to_f32()),
                lanes.rounded(isa, isa.load_u8_as_i32(&octets[1]).to_f32()),
                lanes.rounded(isa, isa.load_u8_as_i32(&octets[2]).to_f32()),
                lanes.rounded(isa, isa.load_u8_as_i32(&octets[3]).to_f32()),
            ];
            isa.narrow_i32_to_u8(ints).store(bytes);
        });
    }
}

/// Sixteen words a step, through two vectors of `f32`.
impl Kernel for AffineRow<'_, u16> {
    type Output = ();

    #[inline(always)]
    fn run<S: Isa>(self, isa: S) {
        let lanes = Lanes::new(isa, self.affine, self.alpha);
        row_groups::in_place::<u16, U16_LANES>(self.row, |words| {
            let ints = isa.load_u16(words).widen();
            let low = lanes.rounded(isa, ints.low.to_f32());
            let high = lanes.rounded(isa, ints.high.to_f32());
            low.narrow_u16(high).store(words);
        });
    }
}

/// Eight samples a step. `f32` storage keeps the affine as the clamp leaves it.
impl Kernel for AffineRow<'_, f32> {
    type Output = ();

    #[inline(always)]
    fn run<S: Isa>(self, isa: S) {
        let lanes = Lanes::new(isa, self.affine, self.alpha);
        row_groups::in_place::<f32, F32_LANES>(self.row, |samples| {
            lanes
                .clamped(lanes.affine(isa.load_f32(samples)))
                .store(samples);
        });
    }
}

/// The affine's constants, lane by lane, set once per row. An alpha lane takes `x · 1 + (−0)`,
/// which keeps every value as it is, a signed zero and a NaN included: a select of the input there
/// would let the compiler turn the select and its store into a masked store, which measured six
/// times slower on Zen 3.
#[derive(Debug, Clone, Copy)]
struct Lanes<F: F32x8> {
    scale: F,
    offset: F,
    /// `max` in every lane: the integer kernels' round bound, an integer as the round takes it,
    /// which leaves an alpha lane's sample as it is.
    max: F,
    /// The clamp's bounds, `∓∞` in an alpha lane. The integer kernels leave them be: the round
    /// saturates to `[0, max]`, which holds every bound's result.
    low: F,
    high: F,
}

impl<F: F32x8> Lanes<F> {
    #[inline(always)]
    fn new<S: Isa<F32 = F>>(isa: S, affine: ChannelAffine, alpha: Alpha) -> Self {
        let ChannelAffine {
            scale,
            offset,
            max,
            low,
            high,
        } = affine;
        let lanes = |color: f32, alpha_lane: f32| match alpha {
            Alpha::Absent => isa.splat_f32(color),
            Alpha::Kept => isa.load_f32(&[
                color, color, color, alpha_lane, color, color, color, alpha_lane,
            ]),
        };
        Self {
            scale: lanes(scale, 1.0),
            offset: lanes(offset, -0.0),
            max: isa.splat_f32(max),
            low: lanes(low, f32::NEG_INFINITY),
            high: lanes(high, f32::INFINITY),
        }
    }

    /// `x · scale + offset`, unclamped.
    #[inline(always)]
    fn affine(self, values: F) -> F {
        values * self.scale + self.offset
    }

    /// `values` held to `[low, high]`: `low > v ? low : v`, then `v > high ? high : v`, the
    /// reference's compare-selects, which keep a NaN.
    #[inline(always)]
    fn clamped(self, values: F) -> F {
        self.low.max(values).min(self.high)
    }

    /// [`Self::affine`] of `values`, clamped to `[0, max]` and rounded, as an integer kernel stores
    /// it.
    #[inline(always)]
    fn rounded<S: Isa<F32 = F>>(self, isa: S, values: F) -> S::I32 {
        isa.round_clamped_f32_to_i32(self.affine(values), self.max)
    }
}
