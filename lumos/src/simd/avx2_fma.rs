//! [`Avx2Fma`]: the `x86_64` [`Isa`], one 256-bit register per vector.
//!
//! Every `unsafe` block here calls an AVX2 or FMA intrinsic, and is sound for one reason: its
//! operands are this module's vector types, whose fields are private, and which only an
//! `Avx2Fma` makes — and an `Avx2Fma` only exists once [`Avx2Fma::detect`] found both features on
//! this CPU. Loads and stores take arrays of exactly one vector's width.

use std::arch::x86_64::*;
use std::ops::{Add, Div, Mul, Sub};

use imaginarium::SimdTier;

use crate::simd::{
    F32_LANES, F32x8, F64_LANES, F64x4, Frexp, Halves, Isa, Kernel, Mask8, lookup_last,
};

/// AVX2 with FMA: every `x86_64` CPU since Haswell (2013) and Excavator (2015).
#[derive(Debug, Clone, Copy)]
pub(crate) struct Avx2Fma {
    _private: (),
}

impl Avx2Fma {
    /// The token, when this CPU has AVX2 and FMA.
    #[inline]
    pub(crate) fn detect() -> Option<Self> {
        SimdTier::Avx2Fma
            .is_supported()
            .then_some(Self { _private: () })
    }

    #[target_feature(enable = "avx2,fma")]
    fn enter<K: Kernel>(self, kernel: K) -> K::Output {
        kernel.run(self)
    }
}

impl Isa for Avx2Fma {
    type F32 = Avx2F32;
    type F64 = Avx2F64;

    #[inline(always)]
    fn run<K: Kernel>(self, kernel: K) -> K::Output {
        // SAFETY: an `Avx2Fma` exists only once `detect` found AVX2 and FMA on this CPU, which is
        // all `enter` needs.
        unsafe { self.enter(kernel) }
    }

    #[inline(always)]
    fn splat_f32(self, value: f32) -> Avx2F32 {
        Avx2F32(unsafe { _mm256_set1_ps(value) })
    }

    #[inline(always)]
    fn splat_f64(self, value: f64) -> Avx2F64 {
        Avx2F64(unsafe { _mm256_set1_pd(value) })
    }

    #[inline(always)]
    fn load_f32(self, lanes: &[f32; F32_LANES]) -> Avx2F32 {
        Avx2F32(unsafe { _mm256_loadu_ps(lanes.as_ptr()) })
    }

    #[inline(always)]
    fn load_f64(self, lanes: &[f64; F64_LANES]) -> Avx2F64 {
        Avx2F64(unsafe { _mm256_loadu_pd(lanes.as_ptr()) })
    }

    #[inline(always)]
    fn lookup_f32(self, table: &[f32], index: Avx2F32) -> Avx2F32 {
        let clamped = index
            .min(self.splat_f32(lookup_last(table)))
            .max(self.splat_f32(0.0));
        Avx2F32(unsafe { _mm256_i32gather_ps::<4>(table.as_ptr(), _mm256_cvttps_epi32(clamped.0)) })
    }
}

/// Eight f32 lanes in one `__m256`.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Avx2F32(__m256);

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
    fn floor(self) -> Self {
        Self(unsafe { _mm256_floor_ps(self.0) })
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

/// Four f64 lanes in one `__m256d`.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Avx2F64(__m256d);

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
            let exponent = _mm256_cvtepi32_epi64(_mm256_cvtpd_epi32(self.0));
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

/// [`Avx2F32`]'s comparison result: each lane all ones or all zeros.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Avx2Mask(__m256);

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
    fn to_bitmask(self) -> u8 {
        unsafe { _mm256_movemask_ps(self.0).cast_unsigned() as u8 }
    }
}
