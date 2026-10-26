//! [`Avx2Mask`]: an [`Avx2F32`] comparison, each lane all ones or all zeros.

use std::arch::x86_64::*;

use crate::backend::avx2_fma::f32x8::Avx2F32;
use crate::mask8::Mask8;

/// [`Avx2F32`]'s comparison result: each lane all ones or all zeros.
#[derive(Debug, Clone, Copy)]
pub struct Avx2Mask(pub(super) __m256);

impl Mask8<Avx2F32> for Avx2Mask {
    #[inline(always)]
    fn select(self, if_true: Avx2F32, if_false: Avx2F32) -> Avx2F32 {
        Avx2F32(unsafe { _mm256_blendv_ps(if_false.0, if_true.0, self.0) })
    }

    #[inline(always)]
    fn keep(self, value: Avx2F32) -> Avx2F32 {
        Avx2F32(unsafe { _mm256_and_ps(self.0, value.0) })
    }

    #[inline(always)]
    fn and(self, other: Self) -> Self {
        Self(unsafe { _mm256_and_ps(self.0, other.0) })
    }

    #[inline(always)]
    fn or(self, other: Self) -> Self {
        Self(unsafe { _mm256_or_ps(self.0, other.0) })
    }

    #[inline(always)]
    fn to_bitmask(self) -> u8 {
        unsafe { _mm256_movemask_ps(self.0).cast_unsigned() as u8 }
    }
}
