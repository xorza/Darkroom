//! [`NeonU16`]: sixteen `u16` lanes in two `uint16x8_t`.

use std::arch::aarch64::*;
use std::ops::Add;

use crate::backend::neon::i32x8::NeonI32;
use crate::backend::neon::u8x32::NeonU8;
use crate::halves::Halves;
use crate::u16x16::{U16_LANES, U16x16};

/// Sixteen `u16` lanes in two `uint16x8_t`: lanes 0–7 and 8–15.
#[derive(Debug, Clone, Copy)]
pub struct NeonU16 {
    pub(super) low: uint16x8_t,
    pub(super) high: uint16x8_t,
}

impl Add for NeonU16 {
    type Output = Self;

    #[inline(always)]
    fn add(self, other: Self) -> Self {
        unsafe {
            Self {
                low: vaddq_u16(self.low, other.low),
                high: vaddq_u16(self.high, other.high),
            }
        }
    }
}

impl U16x16 for NeonU16 {
    type U8 = NeonU8;
    type I32 = NeonI32;

    #[inline(always)]
    fn mul_low(self, other: Self) -> Self {
        unsafe {
            Self {
                low: vmulq_u16(self.low, other.low),
                high: vmulq_u16(self.high, other.high),
            }
        }
    }

    #[inline(always)]
    fn mul_high(self, other: Self) -> Self {
        Self {
            low: mul_high(self.low, other.low),
            high: mul_high(self.high, other.high),
        }
    }

    #[inline(always)]
    fn shift_right<const BITS: i32>(self) -> Self {
        Self {
            low: unsafe { vshrq_n_u16::<BITS>(self.low) },
            high: unsafe { vshrq_n_u16::<BITS>(self.high) },
        }
    }

    #[inline(always)]
    fn widen(self) -> Halves<NeonI32> {
        Halves {
            low: widen(self.low),
            high: widen(self.high),
        }
    }

    #[inline(always)]
    fn narrow_u8(self, high: Self) -> NeonU8 {
        NeonU8 {
            low: self.narrowed(),
            high: high.narrowed(),
        }
    }

    #[inline(always)]
    fn store(self, lanes: &mut [u16; U16_LANES]) {
        let (halves, []) = lanes.as_chunks_mut::<8>() else {
            unreachable!("sixteen lanes are two halves of eight")
        };
        unsafe {
            vst1q_u16(halves[0].as_mut_ptr(), self.low);
            vst1q_u16(halves[1].as_mut_ptr(), self.high);
        }
    }
}

impl NeonU16 {
    /// The sixteen lanes' low eight bits.
    #[inline(always)]
    fn narrowed(self) -> uint8x16_t {
        unsafe { vmovn_high_u16(vmovn_u16(self.low), self.high) }
    }
}

/// The high sixteen bits of each lane's product.
#[inline(always)]
fn mul_high(a: uint16x8_t, b: uint16x8_t) -> uint16x8_t {
    unsafe {
        let low = vmull_u16(vget_low_u16(a), vget_low_u16(b));
        let high = vmull_high_u16(a, b);
        vshrn_high_n_u32::<16>(vshrn_n_u32::<16>(low), high)
    }
}

/// Eight words widened to eight `i32` lanes.
#[inline(always)]
fn widen(words: uint16x8_t) -> NeonI32 {
    unsafe {
        NeonI32 {
            low: vreinterpretq_s32_u32(vmovl_u16(vget_low_u16(words))),
            high: vreinterpretq_s32_u32(vmovl_high_u16(words)),
        }
    }
}
