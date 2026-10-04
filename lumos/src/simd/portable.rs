//! [`Portable`]: the [`Isa`] that is plain lane loops — the fallback on a CPU with no vector Isa,
//! and the bit-exact model every hardware Isa is tested against.

use std::array;
use std::ops::{Add, Div, Mul, Sub};

use crate::simd::{
    F32_LANES, F32x8, F64_LANES, F64x4, Frexp, Halves, Isa, Kernel, Mask8, lookup_lane, lookup_last,
};

/// The Isa every CPU has: each op is a loop over the lanes in Rust's own arithmetic.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Portable {
    _private: (),
}

impl Portable {
    pub(crate) const fn new() -> Self {
        Self { _private: () }
    }
}

impl Isa for Portable {
    type F32 = PortableF32;
    type F64 = PortableF64;

    #[inline(always)]
    fn run<K: Kernel>(self, kernel: K) -> K::Output {
        kernel.run(self)
    }

    #[inline(always)]
    fn splat_f32(self, value: f32) -> PortableF32 {
        PortableF32([value; F32_LANES])
    }

    #[inline(always)]
    fn splat_f64(self, value: f64) -> PortableF64 {
        PortableF64([value; F64_LANES])
    }

    #[inline(always)]
    fn load_f32(self, lanes: &[f32; F32_LANES]) -> PortableF32 {
        PortableF32(*lanes)
    }

    #[inline(always)]
    fn load_f64(self, lanes: &[f64; F64_LANES]) -> PortableF64 {
        PortableF64(*lanes)
    }

    #[inline(always)]
    fn lookup_f32(self, table: &[f32], index: PortableF32) -> PortableF32 {
        let last = lookup_last(table);
        PortableF32(index.0.map(|index| lookup_lane(table, last, index)))
    }
}

/// [`F32x8::max`] on one lane.
#[inline(always)]
const fn compare_swap_max(a: f32, b: f32) -> f32 {
    if a > b { a } else { b }
}

/// [`F32x8::min`] on one lane.
#[inline(always)]
const fn compare_swap_min(a: f32, b: f32) -> f32 {
    if a > b { b } else { a }
}

/// Eight f32 lanes held as an array.
#[derive(Debug, Clone, Copy)]
pub(crate) struct PortableF32([f32; F32_LANES]);

impl PortableF32 {
    #[inline(always)]
    fn zip(self, other: Self, f: impl Fn(f32, f32) -> f32) -> Self {
        Self(array::from_fn(|i| f(self.0[i], other.0[i])))
    }

    #[inline(always)]
    fn compare(self, other: Self, f: impl Fn(f32, f32) -> bool) -> PortableMask {
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
    fn sqrt(self) -> Self {
        Self(self.0.map(f32::sqrt))
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

/// Four f64 lanes held as an array.
#[derive(Debug, Clone, Copy)]
pub(crate) struct PortableF64([f64; F64_LANES]);

impl PortableF64 {
    #[inline(always)]
    fn zip(self, other: Self, f: impl Fn(f64, f64) -> f64) -> Self {
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

/// [`PortableF32`]'s comparison result, one flag per lane.
#[derive(Debug, Clone, Copy)]
pub(crate) struct PortableMask([bool; F32_LANES]);

impl Mask8<PortableF32> for PortableMask {
    #[inline(always)]
    fn select(self, if_true: PortableF32, if_false: PortableF32) -> PortableF32 {
        PortableF32(array::from_fn(|i| {
            if self.0[i] {
                if_true.0[i]
            } else {
                if_false.0[i]
            }
        }))
    }

    #[inline(always)]
    fn keep(self, value: PortableF32) -> PortableF32 {
        PortableF32(array::from_fn(|i| if self.0[i] { value.0[i] } else { 0.0 }))
    }

    #[inline(always)]
    fn to_bitmask(self) -> u8 {
        self.0
            .iter()
            .enumerate()
            .fold(0, |bits, (i, &set)| bits | (u8::from(set) << i))
    }
}
