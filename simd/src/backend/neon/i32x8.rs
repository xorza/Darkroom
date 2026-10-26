//! [`NeonI32`]: eight `i32` lanes in two `int32x4_t`.

use std::arch::aarch64::*;
use std::ops::{Add, BitAnd, Mul};

use crate::backend::neon::f32x8::NeonF32;
use crate::backend::neon::u16x16::NeonU16;
use crate::i32x8::{I32_LANES, I32x8};

/// Eight `i32` lanes in two `int32x4_t`: lanes 0–3 and 4–7.
#[derive(Debug, Clone, Copy)]
pub struct NeonI32 {
    pub(super) low: int32x4_t,
    pub(super) high: int32x4_t,
}

impl Add for NeonI32 {
    type Output = Self;

    #[inline(always)]
    fn add(self, other: Self) -> Self {
        unsafe {
            Self {
                low: vaddq_s32(self.low, other.low),
                high: vaddq_s32(self.high, other.high),
            }
        }
    }
}

impl Mul for NeonI32 {
    type Output = Self;

    #[inline(always)]
    fn mul(self, other: Self) -> Self {
        unsafe {
            Self {
                low: vmulq_s32(self.low, other.low),
                high: vmulq_s32(self.high, other.high),
            }
        }
    }
}

impl BitAnd for NeonI32 {
    type Output = Self;

    #[inline(always)]
    fn bitand(self, other: Self) -> Self {
        unsafe {
            Self {
                low: vandq_s32(self.low, other.low),
                high: vandq_s32(self.high, other.high),
            }
        }
    }
}

impl I32x8 for NeonI32 {
    type F32 = NeonF32;
    type U16 = NeonU16;

    #[inline(always)]
    fn shift_right<const BITS: i32>(self) -> Self {
        Self {
            low: shift_right_logical::<BITS>(self.low),
            high: shift_right_logical::<BITS>(self.high),
        }
    }

    #[inline(always)]
    fn to_f32(self) -> NeonF32 {
        unsafe {
            NeonF32 {
                low: vcvtq_f32_s32(self.low),
                high: vcvtq_f32_s32(self.high),
            }
        }
    }

    #[inline(always)]
    fn narrow_u16(self, high: Self) -> NeonU16 {
        NeonU16 {
            low: self.narrowed(),
            high: high.narrowed(),
        }
    }

    #[inline(always)]
    fn store(self, lanes: &mut [i32; I32_LANES]) {
        let (halves, []) = lanes.as_chunks_mut::<4>() else {
            unreachable!("eight lanes are two halves of four")
        };
        unsafe {
            vst1q_s32(halves[0].as_mut_ptr(), self.low);
            vst1q_s32(halves[1].as_mut_ptr(), self.high);
        }
    }
}

impl NeonI32 {
    /// The eight lanes' bits as f32 lanes.
    #[inline(always)]
    pub(super) fn as_f32(self) -> NeonF32 {
        unsafe {
            NeonF32 {
                low: vreinterpretq_f32_s32(self.low),
                high: vreinterpretq_f32_s32(self.high),
            }
        }
    }

    /// The eight lanes' low sixteen bits.
    #[inline(always)]
    pub(super) fn narrowed(self) -> uint16x8_t {
        unsafe {
            let low = vmovn_u32(vreinterpretq_u32_s32(self.low));
            vmovn_high_u32(low, vreinterpretq_u32_s32(self.high))
        }
    }
}

/// Each lane of `lanes` shifted right by `BITS`, zeros shifted in.
#[inline(always)]
fn shift_right_logical<const BITS: i32>(lanes: int32x4_t) -> int32x4_t {
    unsafe { vreinterpretq_s32_u32(vshrq_n_u32::<BITS>(vreinterpretq_u32_s32(lanes))) }
}
