//! [`NeonU8`]: thirty-two `u8` lanes in two `uint8x16_t`.

use std::arch::aarch64::*;

use crate::backend::neon::u16x16::NeonU16;
use crate::halves::Halves;
use crate::u8x32::{U8_LANES, U8x32};

/// Thirty-two `u8` lanes in two `uint8x16_t`: lanes 0–15 and 16–31.
#[derive(Debug, Clone, Copy)]
pub struct NeonU8 {
    pub(super) low: uint8x16_t,
    pub(super) high: uint8x16_t,
}

impl U8x32 for NeonU8 {
    type U16 = NeonU16;

    #[inline(always)]
    fn widen(self) -> Halves<NeonU16> {
        Halves {
            low: widen(self.low),
            high: widen(self.high),
        }
    }

    #[inline(always)]
    fn store(self, lanes: &mut [u8; U8_LANES]) {
        let (halves, []) = lanes.as_chunks_mut::<16>() else {
            unreachable!("thirty-two lanes are two halves of sixteen")
        };
        unsafe {
            vst1q_u8(halves[0].as_mut_ptr(), self.low);
            vst1q_u8(halves[1].as_mut_ptr(), self.high);
        }
    }
}

/// Sixteen bytes widened to sixteen words.
#[inline(always)]
fn widen(bytes: uint8x16_t) -> NeonU16 {
    unsafe {
        NeonU16 {
            low: vmovl_u8(vget_low_u8(bytes)),
            high: vmovl_high_u8(bytes),
        }
    }
}
