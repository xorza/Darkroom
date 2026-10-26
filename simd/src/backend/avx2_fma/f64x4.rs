//! [`Avx2F64`]: four f64 lanes in one `__m256d`.

use std::arch::x86_64::*;
use std::ops::{Add, Div, Mul, Sub};

use crate::f64x4::{F64_LANES, F64x4};
use crate::frexp::Frexp;

/// Four f64 lanes in one `__m256d`.
#[derive(Debug, Clone, Copy)]
pub struct Avx2F64(pub(super) __m256d);

impl Add for Avx2F64 {
    type Output = Self;

    #[inline(always)]
    fn add(self, other: Self) -> Self {
        Self(unsafe { _mm256_add_pd(self.0, other.0) })
    }
}

impl Sub for Avx2F64 {
    type Output = Self;

    #[inline(always)]
    fn sub(self, other: Self) -> Self {
        Self(unsafe { _mm256_sub_pd(self.0, other.0) })
    }
}

impl Mul for Avx2F64 {
    type Output = Self;

    #[inline(always)]
    fn mul(self, other: Self) -> Self {
        Self(unsafe { _mm256_mul_pd(self.0, other.0) })
    }
}

impl Div for Avx2F64 {
    type Output = Self;

    #[inline(always)]
    fn div(self, other: Self) -> Self {
        Self(unsafe { _mm256_div_pd(self.0, other.0) })
    }
}

impl F64x4 for Avx2F64 {
    #[inline(always)]
    fn mul_add(self, mul: Self, add: Self) -> Self {
        Self(unsafe { _mm256_fmadd_pd(self.0, mul.0, add.0) })
    }

    #[inline(always)]
    fn max(self, other: Self) -> Self {
        Self(unsafe { _mm256_max_pd(self.0, other.0) })
    }

    #[inline(always)]
    fn min(self, other: Self) -> Self {
        Self(unsafe { _mm256_min_pd(other.0, self.0) })
    }

    #[inline(always)]
    fn sqrt(self) -> Self {
        Self(unsafe { _mm256_sqrt_pd(self.0) })
    }

    #[inline(always)]
    fn floor(self) -> Self {
        Self(unsafe { _mm256_floor_pd(self.0) })
    }

    #[inline(always)]
    fn pow2i(self) -> Self {
        unsafe {
            let exponent = _mm256_cvtepi32_epi64(_mm256_cvttpd_epi32(self.0));
            let biased = _mm256_add_epi64(exponent, _mm256_set1_epi64x(1023));
            Self(_mm256_castsi256_pd(_mm256_slli_epi64::<52>(biased)))
        }
    }

    #[inline(always)]
    fn frexp(self) -> Frexp<Self> {
        unsafe {
            let bits = _mm256_castpd_si256(self.0);
            let mantissa = _mm256_or_si256(
                _mm256_and_si256(
                    bits,
                    _mm256_set1_epi64x(0x800f_ffff_ffff_ffff_u64.cast_signed()),
                ),
                _mm256_set1_epi64x(0x3fe0_0000_0000_0000),
            );
            // The top 12 bits as an integer below 2⁵², converted exactly: placed in the mantissa
            // of 2⁵² and that 2⁵² taken off again, as AVX2 has no 64-bit integer conversion.
            let two_52 = _mm256_set1_epi64x(0x4330_0000_0000_0000);
            let field = _mm256_or_si256(_mm256_srli_epi64::<52>(bits), two_52);
            let exponent = _mm256_sub_pd(_mm256_castsi256_pd(field), _mm256_castsi256_pd(two_52));
            Frexp {
                mantissa: Self(_mm256_castsi256_pd(mantissa)),
                exponent: Self(_mm256_sub_pd(exponent, _mm256_set1_pd(1022.0))),
            }
        }
    }

    #[inline(always)]
    fn reduce_sum(self) -> f64 {
        unsafe {
            let pairs = _mm_hadd_pd(
                _mm256_castpd256_pd128(self.0),
                _mm256_extractf128_pd::<1>(self.0),
            );
            _mm_cvtsd_f64(_mm_add_sd(pairs, _mm_unpackhi_pd(pairs, pairs)))
        }
    }

    #[inline(always)]
    fn to_array(self) -> [f64; F64_LANES] {
        let mut lanes = [0.0; F64_LANES];
        unsafe { _mm256_storeu_pd(lanes.as_mut_ptr(), self.0) };
        lanes
    }
}
