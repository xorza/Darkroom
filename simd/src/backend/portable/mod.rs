//! [`Portable`]: the [`Isa`] that is plain lane loops — the fallback on a CPU with no vector Isa,
//! and the bit-exact model every hardware Isa is tested against.

pub(crate) mod f32x8;
pub(crate) mod f64x4;
pub(crate) mod i32x8;
pub(crate) mod mask8;
pub(crate) mod u16x16;
pub(crate) mod u8x32;

use std::array;

use crate::backend::portable::f32x8::PortableF32;
use crate::backend::portable::f64x4::PortableF64;
use crate::backend::portable::i32x8::PortableI32;
use crate::backend::portable::u8x32::PortableU8;
use crate::backend::portable::u16x16::PortableU16;
use crate::f32x8::F32_LANES;
use crate::f64x4::F64_LANES;
use crate::halves::Halves;
use crate::i32x8::I32_LANES;
use crate::isa::Isa;
use crate::u8x32::U8_LANES;
use crate::u16x16::U16_LANES;

/// The Isa every CPU has: each op is a loop over the lanes in Rust's own arithmetic.
#[derive(Debug, Default, Clone, Copy)]
pub struct Portable {
    _private: (),
}

impl Portable {
    pub const fn new() -> Self {
        Self { _private: () }
    }
}

impl Isa for Portable {
    type F32 = PortableF32;
    type F64 = PortableF64;
    type U8 = PortableU8;
    type U16 = PortableU16;
    type I32 = PortableI32;

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
    fn load_u8(self, lanes: &[u8; U8_LANES]) -> PortableU8 {
        PortableU8(*lanes)
    }

    #[inline(always)]
    fn load_u16(self, lanes: &[u16; U16_LANES]) -> PortableU16 {
        PortableU16(*lanes)
    }

    #[inline(always)]
    fn splat_u8(self, value: u8) -> PortableU8 {
        PortableU8([value; U8_LANES])
    }

    #[inline(always)]
    #[expect(
        clippy::cast_possible_truncation,
        reason = "each lane is clamped to [0, high], which i32 holds"
    )]
    fn round_clamped_f32_to_i32(self, values: PortableF32, high: PortableF32) -> PortableI32 {
        debug_assert!(
            high.0
                .iter()
                .all(|&high| is_bound(high.into(), f64::from(1 << 24))),
            "a high of {:?} past integers of at most 2^24",
            high.0
        );
        PortableI32(array::from_fn(|i| {
            let value = values.0[i];
            let clamped = if value > 0.0 {
                value.min(high.0[i])
            } else {
                0.0
            };
            clamped.round_ties_even() as i32
        }))
    }

    #[inline(always)]
    fn load_i32(self, lanes: &[i32; I32_LANES]) -> PortableI32 {
        PortableI32(*lanes)
    }

    #[inline(always)]
    fn splat_u16(self, value: u16) -> PortableU16 {
        PortableU16([value; U16_LANES])
    }

    #[inline(always)]
    fn splat_i32(self, value: i32) -> PortableI32 {
        PortableI32([value; I32_LANES])
    }

    #[inline(always)]
    #[expect(
        clippy::cast_possible_truncation,
        reason = "each lane is clamped to [0, high], which i32 holds"
    )]
    fn round_clamped_f64_to_i32(self, f64s: Halves<PortableF64>, high: PortableF64) -> PortableI32 {
        debug_assert!(
            high.0.iter().all(|&high| is_bound(high, i32::MAX.into())),
            "a high of {:?} past integers of at most i32::MAX",
            high.0
        );
        let halves = [f64s.low.0, f64s.high.0];
        PortableI32(array::from_fn(|i| {
            let value = halves[i / 4][i % 4];
            let high = high.0[i % 4];
            let clamped = if value > 0.0 { value.min(high) } else { 0.0 };
            clamped.round_ties_even() as i32
        }))
    }

    #[inline(always)]
    #[expect(
        clippy::cast_sign_loss,
        reason = "each lane is in [0, 255] by contract"
    )]
    fn narrow_i32_to_u8(self, ints: [PortableI32; 4]) -> PortableU8 {
        PortableU8(array::from_fn(|i| {
            let lane = ints[i / 8].0[i % 8];
            debug_assert!(
                (0..=255).contains(&lane),
                "a lane of {lane} narrowed to a byte"
            );
            lane as u8
        }))
    }
}

/// Whether `high` is an integer in `[0, max]`, as a clamped round's bound.
fn is_bound(high: f64, max: f64) -> bool {
    high.fract() == 0.0 && (0.0..=max).contains(&high)
}

/// [`F32x8::max`](crate::F32x8::max) on one lane.
#[inline(always)]
pub(super) const fn compare_swap_max(a: f32, b: f32) -> f32 {
    if a > b { a } else { b }
}

/// [`F32x8::min`](crate::F32x8::min) on one lane.
#[inline(always)]
pub(super) const fn compare_swap_min(a: f32, b: f32) -> f32 {
    if a > b { b } else { a }
}
