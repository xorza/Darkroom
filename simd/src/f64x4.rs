//! [`F64x4`]: four f64 lanes, each op defined to the bit.

use std::fmt::Debug;
use std::ops::{Add, Div, Mul, Sub};

use crate::frexp::Frexp;

/// f64 lanes in one [`F64x4`], on every Isa: one f32 vector widens into two of these.
pub const F64_LANES: usize = 4;

/// Four f64 lanes, specified as [`F32x8`](crate::f32x8::F32x8) is.
pub trait F64x4:
    Copy + Debug + Add<Output = Self> + Sub<Output = Self> + Mul<Output = Self> + Div<Output = Self>
{
    /// `self · mul + add`, rounded once.
    #[must_use]
    fn mul_add(self, mul: Self, add: Self) -> Self;

    /// `self > other ? self : other`.
    #[must_use]
    fn max(self, other: Self) -> Self;

    /// `self > other ? other : self`.
    #[must_use]
    fn min(self, other: Self) -> Self;

    #[must_use]
    fn sqrt(self) -> Self;

    #[must_use]
    fn floor(self) -> Self;

    /// `2^self` for lanes that hold an integer in `[−1022, 1023]`; other lanes are garbage.
    #[must_use]
    fn pow2i(self) -> Self;

    /// `self = mantissa · 2^exponent`, mantissa in `[0.5, 1)`, for lanes that hold a positive
    /// normal number; every other lane is the same garbage on every Isa, as
    /// [`F32x8::frexp`](crate::f32x8::F32x8::frexp)'s.
    fn frexp(self) -> Frexp<Self>;

    /// `(l0 + l1) + (l2 + l3)`: the fold every Isa takes.
    fn reduce_sum(self) -> f64;

    fn to_array(self) -> [f64; F64_LANES];
}
