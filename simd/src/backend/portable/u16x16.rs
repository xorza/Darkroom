//! [`PortableU16`]: sixteen `u16` lanes held as an array.

use std::array;
use std::ops::Add;

use crate::backend::portable::i32x8::PortableI32;
use crate::backend::portable::u8x32::PortableU8;
use crate::halves::Halves;
use crate::u16x16::{U16_LANES, U16x16};

/// Sixteen `u16` lanes held as an array.
#[derive(Debug, Clone, Copy)]
pub struct PortableU16(pub(super) [u16; U16_LANES]);

impl PortableU16 {
    #[inline(always)]
    fn zip(self, other: Self, f: impl Fn(u16, u16) -> u16) -> Self {
        Self(array::from_fn(|i| f(self.0[i], other.0[i])))
    }
}

impl Add for PortableU16 {
    type Output = Self;

    #[inline(always)]
    fn add(self, other: Self) -> Self {
        self.zip(other, u16::wrapping_add)
    }
}

impl U16x16 for PortableU16 {
    type U8 = PortableU8;
    type I32 = PortableI32;

    #[inline(always)]
    fn mul_low(self, other: Self) -> Self {
        self.zip(other, u16::wrapping_mul)
    }

    #[inline(always)]
    fn mul_high(self, other: Self) -> Self {
        self.zip(other, |a, b| ((u32::from(a) * u32::from(b)) >> 16) as u16)
    }

    #[inline(always)]
    fn shift_right<const BITS: i32>(self) -> Self {
        const { assert!(BITS >= 1 && BITS < 16, "a shift by 1 to 15") };
        Self(self.0.map(|lane| lane >> BITS))
    }

    #[inline(always)]
    fn widen(self) -> Halves<PortableI32> {
        Halves {
            low: PortableI32(array::from_fn(|i| i32::from(self.0[i]))),
            high: PortableI32(array::from_fn(|i| i32::from(self.0[i + 8]))),
        }
    }

    #[inline(always)]
    fn narrow_u8(self, high: Self) -> PortableU8 {
        PortableU8(array::from_fn(|i| {
            let lane = if i < 16 { self.0[i] } else { high.0[i - 16] };
            debug_assert!(lane <= 255, "a lane of {lane} narrowed to a byte");
            lane as u8
        }))
    }

    #[inline(always)]
    fn store(self, lanes: &mut [u16; U16_LANES]) {
        *lanes = self.0;
    }
}
