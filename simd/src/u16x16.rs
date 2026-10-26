//! [`U16x16`]: sixteen `u16` lanes.

use std::fmt::Debug;
use std::ops::Add;

use crate::halves::Halves;
use crate::i32x8::I32x8;
use crate::u8x32::U8x32;

/// `u16` lanes in one [`U16x16`], on every Isa.
pub const U16_LANES: usize = 16;

/// Sixteen `u16` lanes. `+` wraps, as `u16::wrapping_add`.
pub trait U16x16: Copy + Debug + Add<Output = Self> {
    type U8: U8x32<U16 = Self>;
    type I32: I32x8<U16 = Self>;

    /// `self · other`, the low 16 bits, as `u16::wrapping_mul`.
    #[must_use]
    fn mul_low(self, other: Self) -> Self;

    /// `self · other`, the high 16 bits of the 32-bit product.
    #[must_use]
    fn mul_high(self, other: Self) -> Self;

    /// Each lane shifted right by `BITS`, from 1 to 15, zeros shifted in.
    #[must_use]
    fn shift_right<const BITS: i32>(self) -> Self;

    /// Lanes 0–7 and lanes 8–15, each zero-extended to 32 bits.
    fn widen(self) -> Halves<Self::I32>;

    /// `self`'s lanes, then `high`'s, each in `[0, 255]`, as bytes. A lane outside it breaks the
    /// contract, which `Portable` asserts in a debug build; the other Isas then give an unspecified
    /// value. Each Isa narrows with its cheapest instruction.
    fn narrow_u8(self, high: Self) -> Self::U8;

    fn store(self, lanes: &mut [u16; U16_LANES]);
}
