//! [`Avx2I32`]: eight `i32` lanes in one `__m256i`.

use std::arch::x86_64::*;
use std::ops::{Add, BitAnd, Mul};

use crate::backend::avx2_fma::f32x8::Avx2F32;
use crate::backend::avx2_fma::u16x16::Avx2U16;
use crate::i32x8::{I32_LANES, I32x8};

/// Eight `i32` lanes in one `__m256i`.
#[derive(Debug, Clone, Copy)]
pub struct Avx2I32(pub(super) __m256i);

impl Add for Avx2I32 {
    type Output = Self;

    #[inline(always)]
    fn add(self, other: Self) -> Self {
        Self(unsafe { _mm256_add_epi32(self.0, other.0) })
    }
}

impl Mul for Avx2I32 {
    type Output = Self;

    #[inline(always)]
    fn mul(self, other: Self) -> Self {
        Self(unsafe { _mm256_mullo_epi32(self.0, other.0) })
    }
}

impl BitAnd for Avx2I32 {
    type Output = Self;

    #[inline(always)]
    fn bitand(self, other: Self) -> Self {
        Self(unsafe { _mm256_and_si256(self.0, other.0) })
    }
}

impl I32x8 for Avx2I32 {
    type F32 = Avx2F32;
    type U16 = Avx2U16;

    #[inline(always)]
    fn shift_right<const BITS: i32>(self) -> Self {
        Self(unsafe { _mm256_srli_epi32::<BITS>(self.0) })
    }

    /// `cvtdq2ps` rounds under the default round-to-nearest-even mode.
    #[inline(always)]
    fn to_f32(self) -> Avx2F32 {
        Avx2F32(unsafe { _mm256_cvtepi32_ps(self.0) })
    }

    /// `packus` interleaves its two inputs per 128-bit half, which the permute puts back in order.
    #[inline(always)]
    fn narrow_u16(self, high: Self) -> Avx2U16 {
        unsafe {
            let packed = _mm256_packus_epi32(self.0, high.0);
            Avx2U16(_mm256_permute4x64_epi64::<0b11_01_10_00>(packed))
        }
    }

    #[inline(always)]
    fn store(self, lanes: &mut [i32; I32_LANES]) {
        unsafe { _mm256_storeu_si256(lanes.as_mut_ptr().cast(), self.0) }
    }
}
