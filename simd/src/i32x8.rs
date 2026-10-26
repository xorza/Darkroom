//! [`I32x8`]: eight `i32` lanes.

use std::fmt::Debug;
use std::ops::{Add, BitAnd, Mul};

use crate::f32x8::F32x8;
use crate::u16x16::U16x16;

/// `i32` lanes in one [`I32x8`], on every Isa: one per [`F32x8`] lane.
pub const I32_LANES: usize = 8;

/// Eight `i32` lanes. `+` and `*` wrap, as `i32::wrapping_add` and `i32::wrapping_mul`.
pub trait I32x8:
    Copy + Debug + Add<Output = Self> + Mul<Output = Self> + BitAnd<Output = Self>
{
    type F32: F32x8<I32 = Self>;
    type U16: U16x16<I32 = Self>;

    /// Each lane shifted right by `BITS`, from 1 to 31, zeros shifted in.
    #[must_use]
    fn shift_right<const BITS: i32>(self) -> Self;

    /// Each lane as the nearest f32, ties to even, as `i32 as f32`.
    fn to_f32(self) -> Self::F32;

    /// `self`'s lanes, then `high`'s, each in `[0, 65535]`, as `u16`. A lane outside it breaks the
    /// contract, which `Portable` asserts in a debug build; the other Isas then give an unspecified
    /// value. Each Isa narrows with its cheapest instruction.
    fn narrow_u16(self, high: Self) -> Self::U16;

    fn store(self, lanes: &mut [i32; I32_LANES]);
}
