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
use crate::combine::CANCEL_POLL_CHUNK;
use crate::combine::error::StackError;
use crate::combine::pixel_coverage::PixelCoverage;
use crate::frame_store::stored_frame::StoredFrame;
use crate::frame_store::stored_plane::StoredPlane;
use crate::io::cancelled::Cancelled;
use crate::io::image::cfa::CfaType;
use crate::math::size2us::Size2us;
use crate::math::vec2us::Vec2us;

#[derive(Debug)]
pub(crate) struct CommonDomain {
    /// One bit per pixel — a `Vec<bool>` here costs 37.7 MB on a 6K frame against 4.7 MB packed.
    pub(super) valid: BitBuffer2,
    pub(super) sample_count: usize,
}

impl CommonDomain {
    /// The all-valid mask an intersection starts from: one bit per pixel, one row.
    ///
    /// Its own function so the tests can weigh exactly what [`Self::build`] allocates rather than
    /// a copy of the expression that could drift from it.
    fn full_mask(pixel_count: usize) -> BitBuffer2 {
        BitBuffer2::new_filled(Size2us::new(pixel_count, 1), true)
    }

    /// Intersect every frame's coverage into the pixels all of them contribute at.
    pub(super) fn build(
        frames: &[StoredFrame],
        pixel_count: usize,
        cancel: &CancelToken,
    ) -> Result<Self, StackError> {
        let mut common_domain = Self::full_mask(pixel_count);
        for frame in frames {
            Cancelled::check(cancel)?;
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
        Cancelled::check(cancel)?;
        let sample_count = common_domain.count_ones();
        if sample_count == 0 {
            return Err(StackError::NoCommonCoverage);
        }
        Ok(Self {
            valid: common_domain,
            sample_count,
        })
    }

    /// The photosites of `colour` of a `cfa_type` mosaic over an image of `size`, among those
    /// `domain` holds, or among every pixel without one.
    ///
    /// # Errors
    /// [`StackError::NoCommonCoverage`] when the frames share none of them.
    pub(super) fn of_colour(
        domain: Option<&Self>,
        size: Size2us,
        cfa_type: &CfaType,
        colour: u8,
        cancel: &CancelToken,
    ) -> Result<Self, StackError> {
        const BITS: usize = 64;
        let pixel_count = size.pixel_count();
        let mut valid = match domain {
            Some(domain) => domain.valid.clone(),
            None => Self::full_mask(pixel_count),
        };
        let words_per_check = CANCEL_POLL_CHUNK.div_ceil(BITS);
        for (w, word) in valid.words.iter_mut().enumerate() {
            let base = w * BITS;
            if base >= pixel_count {
                break;
            }
            if w % words_per_check == 0 {
                Cancelled::check(cancel)?;
            }
            let mut position = Vec2us::new(base % size.width, base / size.width);
            let mut incoming = 0u64;
            for bit in 0..BITS.min(pixel_count - base) {
                if cfa_type.color_at(position) == colour {
                    incoming |= 1u64 << bit;
                }
                position.x += 1;
                if position.x == size.width {
                    position = Vec2us::new(0, position.y + 1);
                }
            }
            *word &= incoming;
        }
        let sample_count = valid.count_ones();
        if sample_count == 0 {
            return Err(StackError::NoCommonCoverage);
        }
        Ok(Self {
            valid,
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
) -> Result<(), StackError> {
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
            Cancelled::check(cancel)?;
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

#[cfg(test)]
mod tests {
    use crate::combine::normalization::common_domain::CommonDomain;

    /// The common-domain mask is one bit per pixel, not one byte.
    ///
    /// Nothing else would notice a revert to `Vec<bool>`: the combine would still be correct, still
    /// pass every test, and simply hold eight times the mask. At 6144² that is 37.7 MB against 4.7
    /// MB — on top of the frame data the loader is already budgeting for, and invisible to
    /// `load_budget_is_respected_across_configs`, which models frames rather than scratch.
    #[test]
    fn common_domain_mask_stays_one_bit_per_pixel() {
        for pixels in [1024 * 1024usize, 6144 * 6144] {
            let mask = CommonDomain::full_mask(pixels);
            let packed = mask.words.len() * size_of::<u64>();
            let unpacked = pixels * size_of::<bool>();

            // Rows pad to 128 bits, so a single-row mask carries at most 15 bytes of slack.
            assert!(
                packed >= pixels / 8 && packed <= pixels / 8 + 16,
                "{pixels} px: {packed} B is not one bit per pixel (+padding)"
            );
            assert_eq!(
                unpacked / packed,
                8,
                "{pixels} px: packing should be 8x, got {unpacked} B unpacked vs {packed} B packed"
            );
        }
    }
}
