//! [`Avx2U16`]: sixteen `u16` lanes in one `__m256i`.

use std::arch::x86_64::*;
use std::ops::Add;

use crate::backend::avx2_fma::i32x8::Avx2I32;
use crate::backend::avx2_fma::u8x32::Avx2U8;
use crate::halves::Halves;
use crate::u16x16::{U16_LANES, U16x16};

/// Sixteen `u16` lanes in one `__m256i`.
#[derive(Debug, Clone, Copy)]
pub struct Avx2U16(pub(super) __m256i);

impl Add for Avx2U16 {
    type Output = Self;

    #[inline(always)]
    fn add(self, other: Self) -> Self {
        Self(unsafe { _mm256_add_epi16(self.0, other.0) })
    }
}

impl U16x16 for Avx2U16 {
    type U8 = Avx2U8;
    type I32 = Avx2I32;

    #[inline(always)]
    fn mul_low(self, other: Self) -> Self {
        Self(unsafe { _mm256_mullo_epi16(self.0, other.0) })
    }

    #[inline(always)]
    fn mul_high(self, other: Self) -> Self {
        Self(unsafe { _mm256_mulhi_epu16(self.0, other.0) })
    }

    #[inline(always)]
    fn shift_right<const BITS: i32>(self) -> Self {
        Self(unsafe { _mm256_srli_epi16::<BITS>(self.0) })
    }

    #[inline(always)]
    fn widen(self) -> Halves<Avx2I32> {
        unsafe {
            Halves {
                low: Avx2I32(_mm256_cvtepu16_epi32(_mm256_castsi256_si128(self.0))),
                high: Avx2I32(_mm256_cvtepu16_epi32(_mm256_extracti128_si256::<1>(self.0))),
            }
        }
    }

    /// `packus` interleaves its two inputs per 128-bit half, which the permute puts back in
    /// order.
    #[inline(always)]
    fn narrow_u8(self, high: Self) -> Avx2U8 {
        unsafe {
            let packed = _mm256_packus_epi16(self.0, high.0);
            Avx2U8(_mm256_permute4x64_epi64::<0b11_01_10_00>(packed))
        }
    }

    #[inline(always)]
    fn store(self, lanes: &mut [u16; U16_LANES]) {
        unsafe { _mm256_storeu_si256(lanes.as_mut_ptr().cast(), self.0) }
    }
}
