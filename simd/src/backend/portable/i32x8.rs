//! [`PortableI32`]: eight `i32` lanes held as an array.

use std::array;
use std::ops::{Add, BitAnd, Mul};

use crate::backend::portable::f32x8::PortableF32;
use crate::backend::portable::u16x16::PortableU16;
use crate::i32x8::{I32_LANES, I32x8};

/// Eight `i32` lanes held as an array.
#[derive(Debug, Clone, Copy)]
pub struct PortableI32(pub(super) [i32; I32_LANES]);

impl PortableI32 {
    #[inline(always)]
    fn zip(self, other: Self, op: impl Fn(i32, i32) -> i32) -> Self {
        Self(array::from_fn(|i| op(self.0[i], other.0[i])))
    }
}

impl Add for PortableI32 {
    type Output = Self;

    #[inline(always)]
    fn add(self, other: Self) -> Self {
        self.zip(other, i32::wrapping_add)
    }
}

impl Mul for PortableI32 {
    type Output = Self;

    #[inline(always)]
    fn mul(self, other: Self) -> Self {
        self.zip(other, i32::wrapping_mul)
    }
}

impl BitAnd for PortableI32 {
    type Output = Self;

    #[inline(always)]
    fn bitand(self, other: Self) -> Self {
        self.zip(other, |a, b| a & b)
    }
}

impl I32x8 for PortableI32 {
    type F32 = PortableF32;
    type U16 = PortableU16;

    #[inline(always)]
    fn shift_right<const BITS: i32>(self) -> Self {
        const { assert!(BITS >= 1 && BITS < 32, "a shift by 1 to 31") };
        Self(
            self.0
                .map(|lane| (lane.cast_unsigned() >> BITS).cast_signed()),
        )
    }

    #[inline(always)]
    fn to_f32(self) -> PortableF32 {
        PortableF32(self.0.map(|lane| lane as f32))
    }

    #[inline(always)]
    #[expect(
        clippy::cast_sign_loss,
        reason = "each lane is in [0, 65535] by contract"
    )]
    fn narrow_u16(self, high: Self) -> PortableU16 {
        PortableU16(array::from_fn(|i| {
            let lane = if i < 8 { self.0[i] } else { high.0[i - 8] };
            debug_assert!(
                (0..=65535).contains(&lane),
                "a lane of {lane} narrowed to a u16"
            );
            lane as u16
        }))
    }

    #[inline(always)]
    fn store(self, lanes: &mut [i32; I32_LANES]) {
        *lanes = self.0;
    }
}
