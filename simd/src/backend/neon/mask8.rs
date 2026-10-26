//! [`NeonMask`]: a [`NeonF32`] comparison, each lane all ones or all zeros.

use std::arch::aarch64::*;

use crate::backend::neon::f32x8::NeonF32;
use crate::mask8::Mask8;

/// [`NeonF32`]'s comparison result: each lane all ones or all zeros.
#[derive(Debug, Clone, Copy)]
pub struct NeonMask {
    pub(super) low: uint32x4_t,
    pub(super) high: uint32x4_t,
}

impl Mask8<NeonF32> for NeonMask {
    #[inline(always)]
    fn select(self, if_true: NeonF32, if_false: NeonF32) -> NeonF32 {
        unsafe {
            NeonF32 {
                low: vbslq_f32(self.low, if_true.low, if_false.low),
                high: vbslq_f32(self.high, if_true.high, if_false.high),
            }
        }
    }

    #[inline(always)]
    fn keep(self, value: NeonF32) -> NeonF32 {
        NeonF32 {
            low: keep(self.low, value.low),
            high: keep(self.high, value.high),
        }
    }

    #[inline(always)]
    fn and(self, other: Self) -> Self {
        unsafe {
            Self {
                low: vandq_u32(self.low, other.low),
                high: vandq_u32(self.high, other.high),
            }
        }
    }

    #[inline(always)]
    fn or(self, other: Self) -> Self {
        unsafe {
            Self {
                low: vorrq_u32(self.low, other.low),
                high: vorrq_u32(self.high, other.high),
            }
        }
    }

    #[inline(always)]
    fn to_bitmask(self) -> u8 {
        bitmask(self.low) | (bitmask(self.high) << 4)
    }
}

/// `value` where `mask` is set, 0 elsewhere.
#[inline(always)]
fn keep(mask: uint32x4_t, value: float32x4_t) -> float32x4_t {
    unsafe { vreinterpretq_f32_u32(vandq_u32(mask, vreinterpretq_u32_f32(value))) }
}

/// Lane `i` of `mask` at bit `i`.
#[inline(always)]
fn bitmask(mask: uint32x4_t) -> u8 {
    unsafe {
        let weights = vld1q_u32([1, 2, 4, 8].as_ptr());
        vaddvq_u32(vandq_u32(mask, weights)) as u8
    }
}
