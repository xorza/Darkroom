//! The pixels every registered frame actually covers.
//!
//! Normalization compares frames against each other, so it can only measure where all of them have
//! real data: the intersection of the pixels each frame contributes at, by the same
//! [`PixelCoverage`] rule the combine gathers by. Held as one bit per pixel — a `Vec<bool>` costs
//! 37.7 MB on a 6K frame against 4.7 MB packed — and accumulated 64 predicate results at a time, so
//! intersecting is one read-modify-write per word rather than per pixel.
//!
//! Confidence needs no pass of its own: it is positive wherever coverage is, so intersecting
//! `confidence > 0` could only ever remove a pixel the coverage floor had already removed. That is
//! also what lets `source_noise_variance` divide by the confidence at these pixels.

use common::CancelToken;

use crate::bit_buffer2::BitBuffer2;
use crate::math::size2us::Size2us;
use crate::stacking::combine::CANCEL_POLL_CHUNK;
use crate::stacking::combine::error::Error;
use crate::stacking::combine::error::check_cancel;
use crate::stacking::combine::pixel_coverage::PixelCoverage;
use crate::stacking::frame_store::StoredFrame;
use crate::stacking::frame_store::stored_plane::StoredPlane;

#[derive(Debug)]
pub(crate) struct CommonDomain {
    /// One bit per pixel — a `Vec<bool>` here costs 37.7 MB on a 6K frame against 4.7 MB packed.
    pub(super) valid: BitBuffer2,
    pub(super) sample_count: usize,
}

impl CommonDomain {
    /// The all-valid mask an intersection starts from: one bit per pixel, one row.
    ///
    /// Its own function so `mem_budget` can weigh exactly what [`Self::build`] allocates rather
    /// than a copy of the expression that could drift from it.
    pub(crate) fn full_mask(pixel_count: usize) -> BitBuffer2 {
        BitBuffer2::new_filled(Size2us::new(pixel_count, 1), true)
    }

    /// Intersect every frame's coverage into the pixels all of them contribute at.
    pub(super) fn build(
        frames: &[StoredFrame],
        pixel_count: usize,
        cancel: &CancelToken,
    ) -> Result<Self, Error> {
        let mut common_domain = Self::full_mask(pixel_count);
        for frame in frames {
            check_cancel(cancel)?;
            if let Some(coverage) = frame.quality.coverage() {
                intersect_domain(
                    &mut common_domain,
                    coverage,
                    pixel_count,
                    |value| PixelCoverage::new(value).contributes(),
                    cancel,
                )?;
            }
        }
        check_cancel(cancel)?;
        let sample_count = common_domain.count_ones();
        if sample_count == 0 {
            return Err(Error::NoCommonCoverage);
        }
        Ok(Self {
            valid: common_domain,
            sample_count,
        })
    }
}

/// Intersect `common_domain` with the pixels of `plane` that satisfy `is_valid`.
///
/// Accumulates 64 predicate results into a word before touching the mask, so this is one
/// read-modify-write per 64 pixels rather than per pixel.
fn intersect_domain(
    common_domain: &mut BitBuffer2,
    plane: &StoredPlane,
    pixel_count: usize,
    is_valid: impl Fn(f32) -> bool,
    cancel: &CancelToken,
) -> Result<(), Error> {
    const BITS: usize = 64;
    let values = plane.chunk(0, pixel_count);
    // The mask is one row, so word `w` covers pixels `64w..64w+64`.
    let words_per_check = CANCEL_POLL_CHUNK.div_ceil(BITS);
    for (w, word) in common_domain.words.iter_mut().enumerate() {
        let base = w * BITS;
        if base >= pixel_count {
            break;
        }
        if w % words_per_check == 0 {
            check_cancel(cancel)?;
        }
        let mut incoming = 0u64;
        for bit in 0..BITS.min(pixel_count - base) {
            if is_valid(values[base + bit]) {
                incoming |= 1u64 << bit;
            }
        }
        *word &= incoming;
    }
    Ok(())
}
