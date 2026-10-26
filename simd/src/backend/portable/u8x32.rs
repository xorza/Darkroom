//! [`PortableU8`]: thirty-two `u8` lanes held as an array.

use std::array;

use crate::backend::portable::u16x16::PortableU16;
use crate::halves::Halves;
use crate::u8x32::{U8_LANES, U8x32};

/// Thirty-two `u8` lanes held as an array.
#[derive(Debug, Clone, Copy)]
pub struct PortableU8(pub(super) [u8; U8_LANES]);

impl U8x32 for PortableU8 {
    type U16 = PortableU16;

    #[inline(always)]
    fn widen(self) -> Halves<PortableU16> {
        Halves {
            low: PortableU16(array::from_fn(|i| u16::from(self.0[i]))),
            high: PortableU16(array::from_fn(|i| u16::from(self.0[i + 16]))),
        }
    }

    #[inline(always)]
    fn store(self, lanes: &mut [u8; U8_LANES]) {
        *lanes = self.0;
    }
}
