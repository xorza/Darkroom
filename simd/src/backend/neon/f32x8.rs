//! [`NeonF32`]: eight f32 lanes in two `float32x4_t`.

use std::arch::aarch64::*;
use std::ops::{Add, Div, Mul, Sub};

use crate::backend::neon::f64x4::NeonF64;
use crate::backend::neon::i32x8::NeonI32;
use crate::backend::neon::mask8::NeonMask;
use crate::f32x8::{F32_LANES, F32x8};
use crate::frexp::Frexp;
use crate::halves::Halves;

/// Eight f32 lanes in two `float32x4_t`: lanes 0–3 and 4–7.
#[derive(Debug, Clone, Copy)]
pub struct NeonF32 {
    pub(super) low: float32x4_t,
    pub(super) high: float32x4_t,
}

impl Add for NeonF32 {
    type Output = Self;

    #[inline(always)]
    fn add(self, other: Self) -> Self {
        unsafe {
            Self {
                low: vaddq_f32(self.low, other.low),
                high: vaddq_f32(self.high, other.high),
            }
        }
    }
}

impl Sub for NeonF32 {
    type Output = Self;

    #[inline(always)]
    fn sub(self, other: Self) -> Self {
        unsafe {
            Self {
                low: vsubq_f32(self.low, other.low),
                high: vsubq_f32(self.high, other.high),
            }
        }
    }
}

impl Mul for NeonF32 {
    type Output = Self;

    #[inline(always)]
    fn mul(self, other: Self) -> Self {
        unsafe {
            Self {
                low: vmulq_f32(self.low, other.low),
                high: vmulq_f32(self.high, other.high),
            }
        }
    }
}

impl Div for NeonF32 {
    type Output = Self;

    #[inline(always)]
    fn div(self, other: Self) -> Self {
        unsafe {
            Self {
                low: vdivq_f32(self.low, other.low),
                high: vdivq_f32(self.high, other.high),
            }
        }
    }
}

impl F32x8 for NeonF32 {
    type F64 = NeonF64;
    type I32 = NeonI32;
    type Mask = NeonMask;

    #[inline(always)]
    fn mul_add(self, mul: Self, add: Self) -> Self {
        unsafe {
            Self {
                low: vfmaq_f32(add.low, self.low, mul.low),
                high: vfmaq_f32(add.high, self.high, mul.high),
            }
        }
    }

    #[inline(always)]
    fn max(self, other: Self) -> Self {
        unsafe {
            Self {
                low: vbslq_f32(vcgtq_f32(self.low, other.low), self.low, other.low),
                high: vbslq_f32(vcgtq_f32(self.high, other.high), self.high, other.high),
            }
        }
    }

    #[inline(always)]
    fn min(self, other: Self) -> Self {
        unsafe {
            Self {
                low: vbslq_f32(vcgtq_f32(self.low, other.low), other.low, self.low),
                high: vbslq_f32(vcgtq_f32(self.high, other.high), other.high, self.high),
            }
        }
    }

    /// `fmin` is `a > b ? b : a` but where `a` or `b` is NaN, which a NaN `a` makes NaN either
    /// way, and where both are zeros, which a positive bound never is.
    #[inline(always)]
    fn min_positive(self, bound: Self) -> Self {
        unsafe {
            Self {
                low: vminq_f32(self.low, bound.low),
                high: vminq_f32(self.high, bound.high),
            }
        }
    }

    #[inline(always)]
    fn sqrt(self) -> Self {
        unsafe {
            Self {
                low: vsqrtq_f32(self.low),
                high: vsqrtq_f32(self.high),
            }
        }
    }

    #[inline(always)]
    fn abs(self) -> Self {
        unsafe {
            Self {
                low: vabsq_f32(self.low),
                high: vabsq_f32(self.high),
            }
        }
    }

    /// `bsl` takes the sign bit from `sign` and every other bit from the lane.
    #[inline(always)]
    fn copysign(self, sign: Self) -> Self {
        unsafe {
            let sign_bit = vdupq_n_u32(0x8000_0000);
            Self {
                low: vbslq_f32(sign_bit, sign.low, self.low),
                high: vbslq_f32(sign_bit, sign.high, self.high),
            }
        }
    }

    #[inline(always)]
    fn floor(self) -> Self {
        unsafe {
            Self {
                low: vrndmq_f32(self.low),
                high: vrndmq_f32(self.high),
            }
        }
    }

    #[inline(always)]
    fn to_i32(self) -> NeonI32 {
        unsafe {
            NeonI32 {
                low: vcvtq_s32_f32(self.low),
                high: vcvtq_s32_f32(self.high),
            }
        }
    }

    #[inline(always)]
    fn lanes_gt(self, other: Self) -> NeonMask {
        unsafe {
            NeonMask {
                low: vcgtq_f32(self.low, other.low),
                high: vcgtq_f32(self.high, other.high),
            }
        }
    }

    #[inline(always)]
    fn lanes_lt(self, other: Self) -> NeonMask {
        unsafe {
            NeonMask {
                low: vcltq_f32(self.low, other.low),
                high: vcltq_f32(self.high, other.high),
            }
        }
    }

    #[inline(always)]
    fn lanes_eq(self, other: Self) -> NeonMask {
        unsafe {
            NeonMask {
                low: vceqq_f32(self.low, other.low),
                high: vceqq_f32(self.high, other.high),
            }
        }
    }

    /// Pairwise adds spelled out rather than `vaddvq_f32`, whose order the intrinsic does not
    /// promise.
    #[inline(always)]
    fn reduce_sum(self) -> f32 {
        unsafe {
            let halves = vaddq_f32(self.low, self.high);
            let pairs = vpaddq_f32(halves, halves);
            vpadds_f32(vget_low_f32(pairs))
        }
    }

    #[inline(always)]
    fn widen(self) -> Halves<NeonF64> {
        unsafe {
            Halves {
                low: NeonF64 {
                    low: vcvt_f64_f32(vget_low_f32(self.low)),
                    high: vcvt_high_f64_f32(self.low),
                },
                high: NeonF64 {
                    low: vcvt_f64_f32(vget_low_f32(self.high)),
                    high: vcvt_high_f64_f32(self.high),
                },
            }
        }
    }

    #[inline(always)]
    fn frexp(self) -> Frexp<Self> {
        let (low, high) = (split(self.low), split(self.high));
        Frexp {
            mantissa: Self {
                low: low.mantissa,
                high: high.mantissa,
            },
            exponent: Self {
                low: low.exponent,
                high: high.exponent,
            },
        }
    }

    #[inline(always)]
    fn store(self, lanes: &mut [f32; F32_LANES]) {
        let (halves, []) = lanes.as_chunks_mut::<4>() else {
            unreachable!("eight lanes are two halves of four")
        };
        unsafe {
            vst1q_f32(halves[0].as_mut_ptr(), self.low);
            vst1q_f32(halves[1].as_mut_ptr(), self.high);
        }
    }
}

/// [`F32x8::frexp`] of four lanes.
#[inline(always)]
fn split(x: float32x4_t) -> Frexp<float32x4_t> {
    unsafe {
        let bits = vreinterpretq_u32_f32(x);
        let exponent = vsubq_s32(
            vreinterpretq_s32_u32(vshrq_n_u32::<23>(bits)),
            vdupq_n_s32(126),
        );
        let mantissa = vorrq_u32(
            vandq_u32(bits, vdupq_n_u32(0x807f_ffff)),
            vdupq_n_u32(0x3f00_0000),
        );
        Frexp {
            mantissa: vreinterpretq_f32_u32(mantissa),
            exponent: vcvtq_f32_s32(exponent),
        }
    }
}
