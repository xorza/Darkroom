//! [`F32x8`]: eight f32 lanes, each op defined to the bit.

use std::fmt::Debug;
use std::ops::{Add, Div, Mul, Sub};

use crate::f64x4::F64x4;
use crate::frexp::Frexp;
use crate::halves::Halves;
use crate::i32x8::I32x8;
use crate::mask8::Mask8;

/// f32 lanes in one [`F32x8`], on every Isa.
pub const F32_LANES: usize = 8;

/// Eight f32 lanes. Every op is defined per lane to the bit, so every Isa computes the same value:
///
/// - `+ − × ÷` and [`F32x8::sqrt`] are IEEE 754, correctly rounded; [`F32x8::mul_add`] rounds
///   once, on every Isa.
/// - [`F32x8::max`] and [`F32x8::min`] are the two halves of a compare-swap — `a > b ? a : b` and
///   `a > b ? b : a` — so a NaN, or a zero of either sign, decides the same way on every Isa and
///   in a scalar network written with `if a > b { swap }`. `x.max(floor)` sends a NaN `x` to
///   `floor`.
/// - Comparisons are ordered: false where either lane is NaN.
pub trait F32x8:
    Copy + Debug + Add<Output = Self> + Sub<Output = Self> + Mul<Output = Self> + Div<Output = Self>
{
    type F64: F64x4;
    type I32: I32x8<F32 = Self>;
    type Mask: Mask8<Self>;

    /// `self · mul + add`, rounded once.
    #[must_use]
    fn mul_add(self, mul: Self, add: Self) -> Self;

    /// `self > other ? self : other`.
    #[must_use]
    fn max(self, other: Self) -> Self;

    /// `self > other ? other : self`.
    #[must_use]
    fn min(self, other: Self) -> Self;

    /// `self / divisor`, as the divide rounds it, for lanes holding an integer in `[0, divisor]`
    /// and a `divisor` of 255 or 65535, `reciprocal` its reciprocal rounded. A lane past it breaks
    /// the contract, which `Portable` asserts in a debug build; the other Isas then give an
    /// unspecified value. Where a vector divide is slow, as on x86, an Isa takes the reciprocal's
    /// product and one fused correction, `q + r · (v − q · d)`: Markstein's correction of a
    /// correctly rounded reciprocal, which gives the correctly rounded quotient for every such
    /// lane, as the tests check exhaustively.
    #[inline(always)]
    #[must_use]
    fn div_scale(self, divisor: Self, reciprocal: Self) -> Self {
        let _ = reciprocal;
        self / divisor
    }

    /// [`Self::min`] with a `bound` that is positive, `+∞` allowed, in every lane: a NaN in
    /// `self` passes. A NaN or a zero bound breaks the contract, which `Portable` asserts in a
    /// debug build; there the min is unspecified. NEON's own `fmin` is that min only for such a
    /// bound, and costs half the compare and select [`Self::min`] takes.
    #[inline(always)]
    #[must_use]
    fn min_positive(self, bound: Self) -> Self {
        self.min(bound)
    }

    #[must_use]
    fn sqrt(self) -> Self;

    /// Each lane with its sign bit cleared, as `f32::abs`: `−0` to `+0`, a NaN's payload kept.
    #[must_use]
    fn abs(self) -> Self;

    /// The largest integer at or below each lane, exact.
    #[must_use]
    fn floor(self) -> Self;

    /// Each lane, an integer in `[−2³¹, 2³¹)`, as `i32`. A lane past it, a fraction or a NaN breaks
    /// the contract, which `Portable` asserts in a debug build; the other Isas then give an
    /// unspecified value.
    fn to_i32(self) -> Self::I32;

    #[must_use]
    fn lanes_gt(self, other: Self) -> Self::Mask;

    #[must_use]
    fn lanes_lt(self, other: Self) -> Self::Mask;

    #[must_use]
    fn lanes_eq(self, other: Self) -> Self::Mask;

    /// `((l0 + l4) + (l1 + l5)) + ((l2 + l6) + (l3 + l7))`: the fold every Isa takes.
    fn reduce_sum(self) -> f32;

    /// Lanes 0–3 and lanes 4–7, each converted exactly to f64.
    fn widen(self) -> Halves<Self::F64>;

    /// `self = mantissa · 2^exponent`, mantissa in `[0.5, 1)`, for lanes that hold a positive
    /// normal number. Every other lane follows the same bit formula — the exponent field less 126,
    /// and the bits with that field set to 126 — so it is garbage, but the same garbage on every
    /// Isa.
    fn frexp(self) -> Frexp<Self>;

    fn store(self, lanes: &mut [f32; F32_LANES]);

    /// The low `lanes.len()` lanes, at most [`F32_LANES`]; all of them as one store.
    #[inline(always)]
    fn store_partial(self, lanes: &mut [f32]) {
        if let Ok(full) = <&mut [f32; F32_LANES]>::try_from(&mut *lanes) {
            self.store(full);
            return;
        }
        let all = self.to_array();
        lanes.copy_from_slice(&all[..lanes.len()]);
    }

    #[inline(always)]
    fn to_array(self) -> [f32; F32_LANES] {
        let mut lanes = [0.0; F32_LANES];
        self.store(&mut lanes);
        lanes
    }
}
