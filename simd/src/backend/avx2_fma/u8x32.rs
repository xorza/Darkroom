//! [`Avx2U8`]: thirty-two `u8` lanes in one `__m256i`.

use std::arch::x86_64::*;

use crate::backend::avx2_fma::u16x16::Avx2U16;
use crate::halves::Halves;
use crate::u8x32::{U8_LANES, U8x32};

/// Thirty-two `u8` lanes in one `__m256i`.
#[derive(Debug, Clone, Copy)]
pub struct Avx2U8(pub(super) __m256i);

impl U8x32 for Avx2U8 {
    type U16 = Avx2U16;

    #[inline(always)]
    fn widen(self) -> Halves<Avx2U16> {
        unsafe {
            Halves {
                low: Avx2U16(_mm256_cvtepu8_epi16(_mm256_castsi256_si128(self.0))),
                high: Avx2U16(_mm256_cvtepu8_epi16(_mm256_extracti128_si256::<1>(self.0))),
            }
        }
    }

    #[inline(always)]
    fn store(self, lanes: &mut [u8; U8_LANES]) {
        unsafe { _mm256_storeu_si256(lanes.as_mut_ptr().cast(), self.0) }
    }
}
