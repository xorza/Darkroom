//! [`U8x32`]: thirty-two `u8` lanes.

use std::fmt::Debug;

use crate::halves::Halves;
use crate::u16x16::U16x16;

/// `u8` lanes in one [`U8x32`], on every Isa.
pub const U8_LANES: usize = 32;

/// Thirty-two `u8` lanes: 256 bits on every Isa, as [`F32x8`](crate::f32x8::F32x8) is.
pub trait U8x32: Copy + Debug {
    type U16: U16x16<U8 = Self>;

    /// Lanes 0–15 and lanes 16–31, each zero-extended to `u16`.
    fn widen(self) -> Halves<Self::U16>;

    fn store(self, lanes: &mut [u8; U8_LANES]);

    #[inline(always)]
    fn to_array(self) -> [u8; U8_LANES] {
        let mut lanes = [0; U8_LANES];
        self.store(&mut lanes);
        lanes
    }
}
