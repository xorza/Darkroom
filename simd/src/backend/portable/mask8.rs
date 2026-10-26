//! [`PortableMask`]: a [`PortableF32`] comparison, one flag per lane.

use std::array;

use crate::backend::portable::f32x8::PortableF32;
use crate::f32x8::F32_LANES;
use crate::mask8::Mask8;

/// [`PortableF32`]'s comparison result, one flag per lane.
#[derive(Debug, Clone, Copy)]
pub struct PortableMask(pub(super) [bool; F32_LANES]);

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
    fn and(self, other: Self) -> Self {
        Self(array::from_fn(|i| self.0[i] && other.0[i]))
    }

    #[inline(always)]
    fn or(self, other: Self) -> Self {
        Self(array::from_fn(|i| self.0[i] || other.0[i]))
    }

    #[inline(always)]
    fn to_bitmask(self) -> u8 {
        self.0
            .iter()
            .enumerate()
            .fold(0, |bits, (i, &set)| bits | (u8::from(set) << i))
    }
}
