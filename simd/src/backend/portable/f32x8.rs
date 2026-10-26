//! [`PortableF32`]: eight f32 lanes held as an array.

use std::array;
use std::ops::{Add, Div, Mul, Sub};

use crate::backend::portable::f64x4::PortableF64;
use crate::backend::portable::i32x8::PortableI32;
use crate::backend::portable::mask8::PortableMask;
use crate::backend::portable::{compare_swap_max, compare_swap_min};
use crate::f32x8::{F32_LANES, F32x8};
use crate::f64x4::F64_LANES;
use crate::frexp::Frexp;
use crate::halves::Halves;

/// Eight f32 lanes held as an array.
#[derive(Debug, Clone, Copy)]
pub struct PortableF32(pub(super) [f32; F32_LANES]);

impl PortableF32 {
    #[inline(always)]
    pub(super) fn zip(self, other: Self, f: impl Fn(f32, f32) -> f32) -> Self {
        Self(array::from_fn(|i| f(self.0[i], other.0[i])))
    }

    #[inline(always)]
    pub(super) fn compare(self, other: Self, f: impl Fn(f32, f32) -> bool) -> PortableMask {
        PortableMask(array::from_fn(|i| f(self.0[i], other.0[i])))
    }
}

impl Add for PortableF32 {
    type Output = Self;

    #[inline(always)]
    fn add(self, other: Self) -> Self {
        self.zip(other, |a, b| a + b)
    }
}

impl Sub for PortableF32 {
    type Output = Self;

    #[inline(always)]
    fn sub(self, other: Self) -> Self {
        self.zip(other, |a, b| a - b)
    }
}

impl Mul for PortableF32 {
    type Output = Self;

    #[inline(always)]
    fn mul(self, other: Self) -> Self {
        self.zip(other, |a, b| a * b)
    }
}

impl Div for PortableF32 {
    type Output = Self;

    #[inline(always)]
    fn div(self, other: Self) -> Self {
        self.zip(other, |a, b| a / b)
    }
}

impl F32x8 for PortableF32 {
    type F64 = PortableF64;
    type I32 = PortableI32;
    type Mask = PortableMask;

    #[inline(always)]
    fn mul_add(self, mul: Self, add: Self) -> Self {
        Self(array::from_fn(|i| self.0[i].mul_add(mul.0[i], add.0[i])))
    }

    #[inline(always)]
    fn max(self, other: Self) -> Self {
        self.zip(other, compare_swap_max)
    }

    #[inline(always)]
    fn min(self, other: Self) -> Self {
        self.zip(other, compare_swap_min)
    }

    #[inline(always)]
    fn min_positive(self, bound: Self) -> Self {
        debug_assert!(
            bound.0.iter().all(|&bound| bound > 0.0),
            "a bound of {:?} is not positive",
            bound.0
        );
        self.min(bound)
    }

    #[inline(always)]
    fn sqrt(self) -> Self {
        Self(self.0.map(f32::sqrt))
    }

    #[inline(always)]
    fn abs(self) -> Self {
        Self(self.0.map(f32::abs))
    }

    #[inline(always)]
    fn copysign(self, sign: Self) -> Self {
        self.zip(sign, f32::copysign)
    }

    #[inline(always)]
    fn floor(self) -> Self {
        Self(self.0.map(f32::floor))
    }

    #[inline(always)]
    fn to_i32(self) -> PortableI32 {
        PortableI32(self.0.map(|lane| {
            let end = 2.0f32.powi(31);
            debug_assert!(
                lane.floor() == lane && (-end..end).contains(&lane),
                "{lane} is no i32"
            );
            lane as i32
        }))
    }

    #[inline(always)]
    fn lanes_gt(self, other: Self) -> PortableMask {
        self.compare(other, |a, b| a > b)
    }

    #[inline(always)]
    fn lanes_lt(self, other: Self) -> PortableMask {
        self.compare(other, |a, b| a < b)
    }

    #[inline(always)]
    fn lanes_eq(self, other: Self) -> PortableMask {
        self.compare(other, |a, b| a == b)
    }

    #[inline(always)]
    fn reduce_sum(self) -> f32 {
        let l = self.0;
        ((l[0] + l[4]) + (l[1] + l[5])) + ((l[2] + l[6]) + (l[3] + l[7]))
    }

    #[inline(always)]
    fn widen(self) -> Halves<PortableF64> {
        Halves {
            low: PortableF64(array::from_fn(|i| f64::from(self.0[i]))),
            high: PortableF64(array::from_fn(|i| f64::from(self.0[F64_LANES + i]))),
        }
    }

    #[inline(always)]
    fn frexp(self) -> Frexp<Self> {
        Frexp {
            mantissa: Self(
                self.0
                    .map(|x| f32::from_bits((x.to_bits() & 0x807f_ffff) | 0x3f00_0000)),
            ),
            exponent: Self(
                self.0
                    .map(|x| ((x.to_bits() >> 23).cast_signed() - 126) as f32),
            ),
        }
    }

    #[inline(always)]
    fn store(self, lanes: &mut [f32; F32_LANES]) {
        *lanes = self.0;
    }
}
