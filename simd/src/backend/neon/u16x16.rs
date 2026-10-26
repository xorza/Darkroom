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

impl NeonU16 {
    #[inline(always)]
    fn zip(self, other: Self, f: impl Fn(uint16x8_t, uint16x8_t) -> uint16x8_t) -> Self {
        Self {
            low: f(self.low, other.low),
            high: f(self.high, other.high),
        }
    }
}

impl Add for NeonU16 {
    type Output = Self;

    #[inline(always)]
    fn add(self, other: Self) -> Self {
        self.zip(other, |a, b| unsafe { vaddq_u16(a, b) })
    }
}

impl U16x16 for NeonU16 {
    type U8 = NeonU8;
    type I32 = NeonI32;

    #[inline(always)]
    fn mul_low(self, other: Self) -> Self {
        self.zip(other, |a, b| unsafe { vmulq_u16(a, b) })
    }

    #[inline(always)]
    fn mul_high(self, other: Self) -> Self {
        self.zip(other, |a, b| unsafe {
            let low = vmull_u16(vget_low_u16(a), vget_low_u16(b));
            let high = vmull_high_u16(a, b);
            vshrn_high_n_u32::<16>(vshrn_n_u32::<16>(low), high)
        })
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
        let widen = |words: uint16x8_t| unsafe {
            NeonI32 {
                low: vreinterpretq_s32_u32(vmovl_u16(vget_low_u16(words))),
                high: vreinterpretq_s32_u32(vmovl_high_u16(words)),
            }
        };
        Halves {
            low: widen(self.low),
            high: widen(self.high),
        }
    }

    #[inline(always)]
    fn narrow_u8(self, high: Self) -> NeonU8 {
        let narrow = |words: Self| unsafe { vmovn_high_u16(vmovn_u16(words.low), words.high) };
        NeonU8 {
            low: narrow(self),
            high: narrow(high),
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
