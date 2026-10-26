//! [`PortableF64`]: four f64 lanes held as an array.

use std::array;
use std::ops::{Add, Div, Mul, Sub};

use crate::f64x4::{F64_LANES, F64x4};
use crate::frexp::Frexp;

/// Four f64 lanes held as an array.
#[derive(Debug, Clone, Copy)]
pub struct PortableF64(pub(super) [f64; F64_LANES]);

impl PortableF64 {
    #[inline(always)]
    pub(super) fn zip(self, other: Self, f: impl Fn(f64, f64) -> f64) -> Self {
        Self(array::from_fn(|i| f(self.0[i], other.0[i])))
    }
}

impl Add for PortableF64 {
    type Output = Self;

    #[inline(always)]
    fn add(self, other: Self) -> Self {
        self.zip(other, |a, b| a + b)
    }
}

impl Sub for PortableF64 {
    type Output = Self;

    #[inline(always)]
    fn sub(self, other: Self) -> Self {
        self.zip(other, |a, b| a - b)
    }
}

impl Mul for PortableF64 {
    type Output = Self;

    #[inline(always)]
    fn mul(self, other: Self) -> Self {
        self.zip(other, |a, b| a * b)
    }
}

impl Div for PortableF64 {
    type Output = Self;

    #[inline(always)]
    fn div(self, other: Self) -> Self {
        self.zip(other, |a, b| a / b)
    }
}

impl F64x4 for PortableF64 {
    #[inline(always)]
    fn mul_add(self, mul: Self, add: Self) -> Self {
        Self(array::from_fn(|i| self.0[i].mul_add(mul.0[i], add.0[i])))
    }

    #[inline(always)]
    fn max(self, other: Self) -> Self {
        self.zip(other, |a, b| if a > b { a } else { b })
    }

    #[inline(always)]
    fn min(self, other: Self) -> Self {
        self.zip(other, |a, b| if a > b { b } else { a })
    }

    #[inline(always)]
    fn sqrt(self) -> Self {
        Self(self.0.map(f64::sqrt))
    }

    #[inline(always)]
    fn floor(self) -> Self {
        Self(self.0.map(f64::floor))
    }

    #[inline(always)]
    fn pow2i(self) -> Self {
        Self(
            self.0
                .map(|n| f64::from_bits(((n as i64 + 1023) << 52).cast_unsigned())),
        )
    }

    #[inline(always)]
    fn frexp(self) -> Frexp<Self> {
        Frexp {
            mantissa: Self(self.0.map(|x| {
                f64::from_bits((x.to_bits() & 0x800f_ffff_ffff_ffff) | 0x3fe0_0000_0000_0000)
            })),
            exponent: Self(self.0.map(|x| (x.to_bits() >> 52) as f64 - 1022.0)),
        }
    }

    #[inline(always)]
    fn reduce_sum(self) -> f64 {
        let l = self.0;
        (l[0] + l[1]) + (l[2] + l[3])
    }

    #[inline(always)]
    fn to_array(self) -> [f64; F64_LANES] {
        self.0
    }
}
