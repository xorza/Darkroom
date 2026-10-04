//! [`Neon`]: the aarch64 [`Isa`], two 128-bit registers per vector.
//!
//! NEON is part of the aarch64 baseline, so a [`Neon`] needs no detection and [`Isa::run`] no
//! `#[target_feature]` entry, and every intrinsic here is sound to call on any operands. Loads and
//! stores take arrays of exactly one vector's width, split into the two registers' halves.
//!
//! NEON's own `vmaxq`/`vminq` return NaN where either lane is NaN, which is not the compare-swap
//! [`F32x8::max`] specifies, so they are built from a compare and a bit select instead.

use std::arch::aarch64::*;
use std::ops::{Add, Div, Mul, Sub};

use crate::simd::{
    F32_LANES, F32x8, F64_LANES, F64x4, Frexp, Halves, Isa, Kernel, Mask8, lookup_lane, lookup_last,
};

/// NEON, which every aarch64 CPU has.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Neon {
    _private: (),
}

impl Neon {
    pub(crate) const fn new() -> Self {
        Self { _private: () }
    }
}

impl Isa for Neon {
    type F32 = NeonF32;
    type F64 = NeonF64;

    #[inline(always)]
    fn run<K: Kernel>(self, kernel: K) -> K::Output {
        kernel.run(self)
    }

    #[inline(always)]
    fn splat_f32(self, value: f32) -> NeonF32 {
        let lanes = unsafe { vdupq_n_f32(value) };
        NeonF32 {
            low: lanes,
            high: lanes,
        }
    }

    #[inline(always)]
    fn splat_f64(self, value: f64) -> NeonF64 {
        let lanes = unsafe { vdupq_n_f64(value) };
        NeonF64 {
            low: lanes,
            high: lanes,
        }
    }

    #[inline(always)]
    fn load_f32(self, lanes: &[f32; F32_LANES]) -> NeonF32 {
        let (halves, []) = lanes.as_chunks::<4>() else {
            unreachable!("eight lanes are two halves of four")
        };
        unsafe {
            NeonF32 {
                low: vld1q_f32(halves[0].as_ptr()),
                high: vld1q_f32(halves[1].as_ptr()),
            }
        }
    }

    #[inline(always)]
    fn load_f64(self, lanes: &[f64; F64_LANES]) -> NeonF64 {
        let (halves, []) = lanes.as_chunks::<2>() else {
            unreachable!("four lanes are two halves of two")
        };
        unsafe {
            NeonF64 {
                low: vld1q_f64(halves[0].as_ptr()),
                high: vld1q_f64(halves[1].as_ptr()),
            }
        }
    }

    #[inline(always)]
    fn lookup_f32(self, table: &[f32], index: NeonF32) -> NeonF32 {
        let last = lookup_last(table);
        self.load_f32(
            &index
                .to_array()
                .map(|index| lookup_lane(table, last, index)),
        )
    }
}

/// Eight f32 lanes in two `float32x4_t`: lanes 0–3 and 4–7.
#[derive(Debug, Clone, Copy)]
pub(crate) struct NeonF32 {
    low: float32x4_t,
    high: float32x4_t,
}

impl NeonF32 {
    #[inline(always)]
    fn zip(self, other: Self, f: impl Fn(float32x4_t, float32x4_t) -> float32x4_t) -> Self {
        Self {
            low: f(self.low, other.low),
            high: f(self.high, other.high),
        }
    }

    #[inline(always)]
    fn compare(self, other: Self, f: impl Fn(float32x4_t, float32x4_t) -> uint32x4_t) -> NeonMask {
        NeonMask {
            low: f(self.low, other.low),
            high: f(self.high, other.high),
        }
    }
}

impl Add for NeonF32 {
    type Output = Self;

    #[inline(always)]
    fn add(self, other: Self) -> Self {
        self.zip(other, |a, b| unsafe { vaddq_f32(a, b) })
    }
}

impl Sub for NeonF32 {
    type Output = Self;

    #[inline(always)]
    fn sub(self, other: Self) -> Self {
        self.zip(other, |a, b| unsafe { vsubq_f32(a, b) })
    }
}

impl Mul for NeonF32 {
    type Output = Self;

    #[inline(always)]
    fn mul(self, other: Self) -> Self {
        self.zip(other, |a, b| unsafe { vmulq_f32(a, b) })
    }
}

impl Div for NeonF32 {
    type Output = Self;

    #[inline(always)]
    fn div(self, other: Self) -> Self {
        self.zip(other, |a, b| unsafe { vdivq_f32(a, b) })
    }
}

impl F32x8 for NeonF32 {
    type F64 = NeonF64;
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
        self.zip(other, |a, b| unsafe { vbslq_f32(vcgtq_f32(a, b), a, b) })
    }

    #[inline(always)]
    fn min(self, other: Self) -> Self {
        self.zip(other, |a, b| unsafe { vbslq_f32(vcgtq_f32(a, b), b, a) })
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
    fn lanes_gt(self, other: Self) -> NeonMask {
        self.compare(other, |a, b| unsafe { vcgtq_f32(a, b) })
    }

    #[inline(always)]
    fn lanes_lt(self, other: Self) -> NeonMask {
        self.compare(other, |a, b| unsafe { vcltq_f32(a, b) })
    }

    #[inline(always)]
    fn lanes_eq(self, other: Self) -> NeonMask {
        self.compare(other, |a, b| unsafe { vceqq_f32(a, b) })
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
        let split = |x: float32x4_t| unsafe {
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
        };
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

/// Four f64 lanes in two `float64x2_t`: lanes 0–1 and 2–3.
#[derive(Debug, Clone, Copy)]
pub(crate) struct NeonF64 {
    low: float64x2_t,
    high: float64x2_t,
}

impl NeonF64 {
    #[inline(always)]
    fn zip(self, other: Self, f: impl Fn(float64x2_t, float64x2_t) -> float64x2_t) -> Self {
        Self {
            low: f(self.low, other.low),
            high: f(self.high, other.high),
        }
    }

    #[inline(always)]
    fn map(self, f: impl Fn(float64x2_t) -> float64x2_t) -> Self {
        Self {
            low: f(self.low),
            high: f(self.high),
        }
    }
}

impl Add for NeonF64 {
    type Output = Self;

    #[inline(always)]
    fn add(self, other: Self) -> Self {
        self.zip(other, |a, b| unsafe { vaddq_f64(a, b) })
    }
}

impl Sub for NeonF64 {
    type Output = Self;

    #[inline(always)]
    fn sub(self, other: Self) -> Self {
        self.zip(other, |a, b| unsafe { vsubq_f64(a, b) })
    }
}

impl Mul for NeonF64 {
    type Output = Self;

    #[inline(always)]
    fn mul(self, other: Self) -> Self {
        self.zip(other, |a, b| unsafe { vmulq_f64(a, b) })
    }
}

impl Div for NeonF64 {
    type Output = Self;

    #[inline(always)]
    fn div(self, other: Self) -> Self {
        self.zip(other, |a, b| unsafe { vdivq_f64(a, b) })
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
        self.zip(other, |a, b| unsafe { vbslq_f64(vcgtq_f64(a, b), a, b) })
    }

    #[inline(always)]
    fn min(self, other: Self) -> Self {
        self.zip(other, |a, b| unsafe { vbslq_f64(vcgtq_f64(a, b), b, a) })
    }

    #[inline(always)]
    fn sqrt(self) -> Self {
        self.map(|a| unsafe { vsqrtq_f64(a) })
    }

    #[inline(always)]
    fn floor(self) -> Self {
        self.map(|a| unsafe { vrndmq_f64(a) })
    }

    #[inline(always)]
    fn pow2i(self) -> Self {
        self.map(|n| unsafe {
            let biased = vaddq_s64(vcvtq_s64_f64(n), vdupq_n_s64(1023));
            vreinterpretq_f64_s64(vshlq_n_s64::<52>(biased))
        })
    }

    #[inline(always)]
    fn frexp(self) -> Frexp<Self> {
        let mantissa = self.map(|x| unsafe {
            let bits = vreinterpretq_u64_f64(x);
            vreinterpretq_f64_u64(vorrq_u64(
                vandq_u64(bits, vdupq_n_u64(0x800f_ffff_ffff_ffff)),
                vdupq_n_u64(0x3fe0_0000_0000_0000),
            ))
        });
        let exponent = self.map(|x| unsafe {
            let field = vcvtq_f64_u64(vshrq_n_u64::<52>(vreinterpretq_u64_f64(x)));
            vsubq_f64(field, vdupq_n_f64(1022.0))
        });
        Frexp { mantissa, exponent }
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

/// [`NeonF32`]'s comparison result: each lane all ones or all zeros.
#[derive(Debug, Clone, Copy)]
pub(crate) struct NeonMask {
    low: uint32x4_t,
    high: uint32x4_t,
}

impl Mask8<NeonF32> for NeonMask {
    #[inline(always)]
    fn select(self, if_true: NeonF32, if_false: NeonF32) -> NeonF32 {
        unsafe {
            NeonF32 {
                low: vbslq_f32(self.low, if_true.low, if_false.low),
                high: vbslq_f32(self.high, if_true.high, if_false.high),
            }
        }
    }

    #[inline(always)]
    fn keep(self, value: NeonF32) -> NeonF32 {
        let and = |mask: uint32x4_t, value: float32x4_t| unsafe {
            vreinterpretq_f32_u32(vandq_u32(mask, vreinterpretq_u32_f32(value)))
        };
        NeonF32 {
            low: and(self.low, value.low),
            high: and(self.high, value.high),
        }
    }

    #[inline(always)]
    fn to_bitmask(self) -> u8 {
        let bits = |mask: uint32x4_t| unsafe {
            let weights = vld1q_u32([1, 2, 4, 8].as_ptr());
            vaddvq_u32(vandq_u32(mask, weights)) as u8
        };
        bits(self.low) | (bits(self.high) << 4)
    }
}
