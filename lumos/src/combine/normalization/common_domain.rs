//! The pixels every registered frame actually covers.
//!
//! Normalization compares frames against each other, so it can only measure where all of them have
//! real data: the intersection of the pixels each frame contributes at, by the same [`FrameGate`]
//! the combine gathers by. Held as one bit per pixel — a `Vec<bool>` costs
//! 37.7 MB on a 6K frame against 4.7 MB packed — and accumulated 64 predicate results at a time, so
//! intersecting is one read-modify-write per word rather than per pixel.
//!
//! Confidence needs no pass of its own: it is positive wherever coverage is, so intersecting
//! `confidence > 0` could only ever remove a pixel the coverage floor had already removed. That is
//! also what lets `source_noise_variance` divide by the confidence at these pixels.

use common::CancelToken;
use rayon::prelude::*;

use crate::bit_buffer2::BitBuffer2;
use crate::combine::cache::frame_gate::FrameGate;
use crate::combine::error::StackError;
use crate::frame_store::stored_frame::StoredFrame;
use crate::io::cancelled::Cancelled;
use crate::io::image::cfa::CfaType;
use crate::math::size2us::Size2us;
use crate::math::vec2us::Vec2us;

/// Pixels per word of the mask, which is one row: word `w` covers pixels `64w..64w + 64`.
pub(super) const WORD_BITS: usize = u64::BITS as usize;

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

    /// The bytes a domain over `pixel_count` pixels holds: its mask.
    pub(crate) fn heap_bytes(pixel_count: usize) -> usize {
        BitBuffer2::heap_bytes(Size2us::new(pixel_count, 1))
    }

    /// Intersect every frame's coverage of `channel` into the pixels all of them contribute at.
    ///
    /// In parallel over spans of the mask, each reading its span of every frame: AND is
    /// order-independent, so the mask is the serial one, and only one mask is ever held.
    pub(super) fn build(
        frames: &[&StoredFrame],
        channel: usize,
        pixel_count: usize,
        cancel: &CancelToken,
    ) -> Result<Self, StackError> {
        let mut common_domain = Self::full_mask(pixel_count);
        common_domain
            .words
            .par_chunks_mut(Cancelled::POLL_CHUNK / WORD_BITS)
            .enumerate()
            .try_for_each(|(span, words)| {
                intersect_frames(frames, channel, pixel_count, span, words, cancel)
            })?;
        Self::of_mask(common_domain)
    }

    /// [`Self::build`] one span after another, for a caller already parallel over its jobs: a job
    /// that waited on a nested parallel loop would let its worker take up another job meanwhile,
    /// and the jobs alive at once, each holding its scratch, would outnumber the workers.
    pub(super) fn build_in_job(
        frames: &[&StoredFrame],
        channel: usize,
        pixel_count: usize,
        cancel: &CancelToken,
    ) -> Result<Self, StackError> {
        let mut common_domain = Self::full_mask(pixel_count);
        for (span, words) in common_domain
            .words
            .chunks_mut(Cancelled::POLL_CHUNK / WORD_BITS)
            .enumerate()
        {
            intersect_frames(frames, channel, pixel_count, span, words, cancel)?;
        }
        Self::of_mask(common_domain)
    }

    /// The domain `valid` holds; [`StackError::NoCommonCoverage`] when it holds no pixel.
    fn of_mask(valid: BitBuffer2) -> Result<Self, StackError> {
        let sample_count = valid.count_ones();
        if sample_count == 0 {
            return Err(StackError::NoCommonCoverage);
        }
        Ok(Self {
            valid,
            sample_count,
        })
    }

    /// The photosites of `colour` of a `cfa_type` mosaic over an image of `size` among those
    /// `domain` holds.
    ///
    /// # Errors
    /// [`StackError::NoCommonCoverage`] when the frames share none of them.
    pub(super) fn of_colour(
        domain: &Self,
        size: Size2us,
        cfa_type: &CfaType,
        colour: usize,
        cancel: &CancelToken,
    ) -> Result<Self, StackError> {
        let pixel_count = size.pixel_count();
        let mut valid = domain.valid.clone();
        valid
            .words
            .par_chunks_mut(Cancelled::POLL_CHUNK / WORD_BITS)
            .enumerate()
            .try_for_each(|(span, words)| {
                Cancelled::check(cancel)?;
                let base = span * Cancelled::POLL_CHUNK;
                if base < pixel_count {
                    let end = (base + words.len() * WORD_BITS).min(pixel_count);
                    intersect_span(words, end - base, |index| {
                        let pixel = base + index;
                        let position = Vec2us::new(pixel % size.width, pixel / size.width);
                        cfa_type.color_at(position) == colour
                    });
                }
                Ok::<_, StackError>(())
            })?;
        Self::of_mask(valid)
    }
}

/// Intersect span `span` of a mask over `pixel_count` pixels, its `words`, with every frame's
/// coverage of `channel` there.
fn intersect_frames(
    frames: &[&StoredFrame],
    channel: usize,
    pixel_count: usize,
    span: usize,
    words: &mut [u64],
    cancel: &CancelToken,
) -> Result<(), StackError> {
    Cancelled::check(cancel)?;
    let base = span * Cancelled::POLL_CHUNK;
    if base < pixel_count {
        let end = (base + words.len() * WORD_BITS).min(pixel_count);
        for frame in frames {
            let gate = FrameGate::of(frame, channel, base, end);
            if !gate.everywhere() {
                intersect_span(words, end - base, |index| gate.sample(index).is_some());
            }
        }
    }
    Ok(())
}

/// Intersect `words`, the mask over `pixels` pixels from a word's start, with the pixels `is_valid`
/// names by their index from that start; past the last pixel a word's bits are cleared.
///
/// Accumulates 64 predicate results into a word before touching the mask, so this is one
/// read-modify-write per 64 pixels rather than per pixel.
fn intersect_span(words: &mut [u64], pixels: usize, is_valid: impl Fn(usize) -> bool) {
    for (w, word) in words.iter_mut().enumerate() {
        let base = w * WORD_BITS;
        if base >= pixels {
            break;
        }
        let mut incoming = 0u64;
        for bit in 0..WORD_BITS.min(pixels - base) {
            if is_valid(base + bit) {
                incoming |= 1u64 << bit;
            }
        }
        *word &= incoming;
    }
}

#[cfg(test)]
mod tests {
    use crate::combine::normalization::common_domain::CommonDomain;

    /// The common-domain mask is one bit per pixel, not one byte, and [`CommonDomain::heap_bytes`]
    /// is what it holds.
    ///
    /// Nothing else would notice a revert to `Vec<bool>` at the sizes the tests run: the combine
    /// would still be correct and simply hold eight times the mask, 37.7 MB against 4.7 MB at 6144²,
    /// beside the frame data the plan charges.
    #[test]
    fn common_domain_mask_stays_one_bit_per_pixel() {
        for pixels in [1024 * 1024usize, 6144 * 6144] {
            let mask = CommonDomain::full_mask(pixels);
            let packed = mask.words.len() * size_of::<u64>();
            let unpacked = pixels * size_of::<bool>();
            assert_eq!(CommonDomain::heap_bytes(pixels), packed, "{pixels} px");

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
