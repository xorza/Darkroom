//! [`NeonF64`]: four f64 lanes in two `float64x2_t`.

use std::arch::aarch64::*;
use std::ops::{Add, Div, Mul, Sub};

use crate::f64x4::{F64_LANES, F64x4};
use crate::frexp::Frexp;

/// Four f64 lanes in two `float64x2_t`: lanes 0–1 and 2–3.
#[derive(Debug, Clone, Copy)]
pub struct NeonF64 {
    pub(super) low: float64x2_t,
    pub(super) high: float64x2_t,
}

impl Add for NeonF64 {
    type Output = Self;

    #[inline(always)]
    fn add(self, other: Self) -> Self {
        unsafe {
            Self {
                low: vaddq_f64(self.low, other.low),
                high: vaddq_f64(self.high, other.high),
            }
        }
    }
}

impl Sub for NeonF64 {
    type Output = Self;

    #[inline(always)]
    fn sub(self, other: Self) -> Self {
        unsafe {
            Self {
                low: vsubq_f64(self.low, other.low),
                high: vsubq_f64(self.high, other.high),
            }
        }
    }
}

impl Mul for NeonF64 {
    type Output = Self;

    #[inline(always)]
    fn mul(self, other: Self) -> Self {
        unsafe {
            Self {
                low: vmulq_f64(self.low, other.low),
                high: vmulq_f64(self.high, other.high),
            }
        }
    }
}

impl Div for NeonF64 {
    type Output = Self;

    #[inline(always)]
    fn div(self, other: Self) -> Self {
        unsafe {
            Self {
                low: vdivq_f64(self.low, other.low),
                high: vdivq_f64(self.high, other.high),
            }
        }
    }
}

impl F64x4 for NeonF64 {
    #[inline(always)]
    fn mul_add(self, mul: Self, add: Self) -> Self {
        unsafe {
            Self {
                low: vfmaq_f64(add.low, self.low, mul.low),
                high: vfmaq_f64(add.high, self.high, mul.high),
            }
        }
    }

    #[inline(always)]
    fn max(self, other: Self) -> Self {
        unsafe {
            Self {
                low: vbslq_f64(vcgtq_f64(self.low, other.low), self.low, other.low),
                high: vbslq_f64(vcgtq_f64(self.high, other.high), self.high, other.high),
            }
        }
    }

    #[inline(always)]
    fn min(self, other: Self) -> Self {
        unsafe {
            Self {
                low: vbslq_f64(vcgtq_f64(self.low, other.low), other.low, self.low),
                high: vbslq_f64(vcgtq_f64(self.high, other.high), other.high, self.high),
            }
        }
    }

    #[inline(always)]
    fn sqrt(self) -> Self {
        unsafe {
            Self {
                low: vsqrtq_f64(self.low),
                high: vsqrtq_f64(self.high),
            }
        }
    }

    #[inline(always)]
    fn floor(self) -> Self {
        unsafe {
            Self {
                low: vrndmq_f64(self.low),
                high: vrndmq_f64(self.high),
            }
        }
    }

    #[inline(always)]
    fn pow2i(self) -> Self {
        Self {
            low: pow2i(self.low),
            high: pow2i(self.high),
        }
    }

    #[inline(always)]
    fn frexp(self) -> Frexp<Self> {
        Frexp {
            mantissa: Self {
                low: mantissa(self.low),
                high: mantissa(self.high),
            },
            exponent: Self {
                low: exponent(self.low),
                high: exponent(self.high),
            },
        }
    }

    #[inline(always)]
    fn reduce_sum(self) -> f64 {
        unsafe { vpaddd_f64(self.low) + vpaddd_f64(self.high) }
    }

    #[inline(always)]
    fn to_array(self) -> [f64; F64_LANES] {
        let mut lanes = [0.0; F64_LANES];
        let (halves, []) = lanes.as_chunks_mut::<2>() else {
            unreachable!("four lanes are two halves of two")
        };
        unsafe {
            vst1q_f64(halves[0].as_mut_ptr(), self.low);
            vst1q_f64(halves[1].as_mut_ptr(), self.high);
        }
        lanes
    }
}

/// `2^n` of each lane's integer `n`, built in the exponent field.
#[inline(always)]
fn pow2i(n: float64x2_t) -> float64x2_t {
    unsafe {
        let biased = vaddq_s64(vcvtq_s64_f64(n), vdupq_n_s64(1023));
        vreinterpretq_f64_s64(vshlq_n_s64::<52>(biased))
    }
}

/// [`F64x4::frexp`]'s mantissa of each lane: its exponent field set to that of `[0.5, 1)`.
#[inline(always)]
fn mantissa(x: float64x2_t) -> float64x2_t {
    unsafe {
        let bits = vreinterpretq_u64_f64(x);
        vreinterpretq_f64_u64(vorrq_u64(
            vandq_u64(bits, vdupq_n_u64(0x800f_ffff_ffff_ffff)),
            vdupq_n_u64(0x3fe0_0000_0000_0000),
        ))
    }
}

/// [`F64x4::frexp`]'s exponent of each lane, read off its exponent field.
#[inline(always)]
fn exponent(x: float64x2_t) -> float64x2_t {
    unsafe {
        let field = vcvtq_f64_u64(vshrq_n_u64::<52>(vreinterpretq_u64_f64(x)));
        vsubq_f64(field, vdupq_n_f64(1022.0))
    }
}
