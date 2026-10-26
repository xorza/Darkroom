//! [`Avx2F32`]: eight f32 lanes in one `__m256`.

use std::arch::x86_64::*;
use std::ops::{Add, Div, Mul, Sub};

use crate::backend::avx2_fma::f64x4::Avx2F64;
use crate::backend::avx2_fma::i32x8::Avx2I32;
use crate::backend::avx2_fma::mask8::Avx2Mask;
use crate::f32x8::{F32_LANES, F32x8};
use crate::frexp::Frexp;
use crate::halves::Halves;

/// Eight f32 lanes in one `__m256`.
#[derive(Debug, Clone, Copy)]
pub struct Avx2F32(pub(super) __m256);

impl Add for Avx2F32 {
    type Output = Self;

    #[inline(always)]
    fn add(self, other: Self) -> Self {
        Self(unsafe { _mm256_add_ps(self.0, other.0) })
    }
}

impl Sub for Avx2F32 {
    type Output = Self;

    #[inline(always)]
    fn sub(self, other: Self) -> Self {
        Self(unsafe { _mm256_sub_ps(self.0, other.0) })
    }
}

impl Mul for Avx2F32 {
    type Output = Self;

    #[inline(always)]
    fn mul(self, other: Self) -> Self {
        Self(unsafe { _mm256_mul_ps(self.0, other.0) })
    }
}

impl Div for Avx2F32 {
    type Output = Self;

    #[inline(always)]
    fn div(self, other: Self) -> Self {
        Self(unsafe { _mm256_div_ps(self.0, other.0) })
    }
}

impl F32x8 for Avx2F32 {
    type F64 = Avx2F64;
    type I32 = Avx2I32;
    type Mask = Avx2Mask;

    #[inline(always)]
    fn mul_add(self, mul: Self, add: Self) -> Self {
        Self(unsafe { _mm256_fmadd_ps(self.0, mul.0, add.0) })
    }

    /// `maxps a, b` is `a > b ? a : b` exactly.
    #[inline(always)]
    fn max(self, other: Self) -> Self {
        Self(unsafe { _mm256_max_ps(self.0, other.0) })
    }

    /// `minps b, a` is `b < a ? b : a`, which is `a > b ? b : a`.
    #[inline(always)]
    fn min(self, other: Self) -> Self {
        Self(unsafe { _mm256_min_ps(other.0, self.0) })
    }

    #[inline(always)]
    fn sqrt(self) -> Self {
        Self(unsafe { _mm256_sqrt_ps(self.0) })
    }

    #[inline(always)]
    fn abs(self) -> Self {
        Self(unsafe { _mm256_andnot_ps(_mm256_set1_ps(-0.0), self.0) })
    }

    #[inline(always)]
    fn copysign(self, sign: Self) -> Self {
        unsafe {
            let sign_bit = _mm256_set1_ps(-0.0);
            Self(_mm256_or_ps(
                _mm256_andnot_ps(sign_bit, self.0),
                _mm256_and_ps(sign_bit, sign.0),
            ))
        }
    }

    #[inline(always)]
    fn floor(self) -> Self {
        Self(unsafe { _mm256_floor_ps(self.0) })
    }

    #[inline(always)]
    fn to_i32(self) -> Avx2I32 {
        Avx2I32(unsafe { _mm256_cvttps_epi32(self.0) })
    }

    #[inline(always)]
    fn lanes_gt(self, other: Self) -> Avx2Mask {
        Avx2Mask(unsafe { _mm256_cmp_ps::<_CMP_GT_OQ>(self.0, other.0) })
    }

    #[inline(always)]
    fn lanes_lt(self, other: Self) -> Avx2Mask {
        Avx2Mask(unsafe { _mm256_cmp_ps::<_CMP_LT_OQ>(self.0, other.0) })
    }

    #[inline(always)]
    fn lanes_eq(self, other: Self) -> Avx2Mask {
        Avx2Mask(unsafe { _mm256_cmp_ps::<_CMP_EQ_OQ>(self.0, other.0) })
    }

    #[inline(always)]
    fn reduce_sum(self) -> f32 {
        unsafe {
            let halves = _mm_add_ps(
                _mm256_castps256_ps128(self.0),
                _mm256_extractf128_ps::<1>(self.0),
            );
            let pairs = _mm_add_ps(halves, _mm_movehdup_ps(halves));
            _mm_cvtss_f32(_mm_add_ss(pairs, _mm_movehl_ps(pairs, pairs)))
        }
    }

    #[inline(always)]
    fn widen(self) -> Halves<Avx2F64> {
        unsafe {
            Halves {
                low: Avx2F64(_mm256_cvtps_pd(_mm256_castps256_ps128(self.0))),
                high: Avx2F64(_mm256_cvtps_pd(_mm256_extractf128_ps::<1>(self.0))),
            }
        }
    }

    #[inline(always)]
    fn frexp(self) -> Frexp<Self> {
        unsafe {
            let bits = _mm256_castps_si256(self.0);
            let exponent = _mm256_sub_epi32(_mm256_srli_epi32::<23>(bits), _mm256_set1_epi32(126));
            let mantissa = _mm256_or_si256(
                _mm256_and_si256(bits, _mm256_set1_epi32(0x807f_ffff_u32.cast_signed())),
                _mm256_set1_epi32(0x3f00_0000),
            );
            Frexp {
                mantissa: Self(_mm256_castsi256_ps(mantissa)),
                exponent: Self(_mm256_cvtepi32_ps(exponent)),
            }
        }
    }

    #[inline(always)]
    fn store(self, lanes: &mut [f32; F32_LANES]) {
        unsafe { _mm256_storeu_ps(lanes.as_mut_ptr(), self.0) }
    }
}
