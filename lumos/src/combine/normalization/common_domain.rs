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
//! also what lets a slot's noise variance divide by the confidence at these pixels.

use std::slice;

use common::CancelToken;
use rayon::prelude::*;

use crate::bit_buffer2::BitBuffer2;
use crate::combine::error::StackError;
use crate::combine::frame_cache::frame_gate::FrameGate;
use crate::error::Cancelled;
use crate::frame_store::stored_frame::StoredFrame;
use crate::frame_store::stratified_samples::SAMPLE_LIMIT;
use crate::math::size2us::Size2us;
use crate::math::vec2us::Vec2us;

/// Pixels per word of the mask, which is one row: word `w` covers pixels `64w..64w + 64`.
const WORD_BITS: usize = u64::BITS as usize;

const _: () = assert!(
    Cancelled::POLL_CHUNK.is_multiple_of(WORD_BITS),
    "a gather chunk starts on a mask word"
);

#[derive(Debug)]
pub(crate) struct CommonDomain {
    /// One bit per pixel — a `Vec<bool>` here costs 37.7 MB on a 6K frame against 4.7 MB packed.
    pub(super) valid: BitBuffer2,
    pub(super) sample_count: usize,
}

impl CommonDomain {
    /// The all-valid mask an intersection starts from: one bit per pixel, one row, its padding
    /// clear. The walks over a domain read its words whole, and a span every frame covers is never
    /// intersected, so a padding bit left set would read as a pixel past the plane.
    ///
    /// Its own function so the tests can weigh exactly what [`Self::build`] allocates rather than
    /// a copy of the expression that could drift from it.
    fn full_mask(pixel_count: usize) -> BitBuffer2 {
        let mut mask = BitBuffer2::new_filled(Size2us::new(pixel_count, 1), true);
        mask.clear_padding();
        mask
    }

    /// The bytes a domain over `pixel_count` pixels holds: its mask.
    pub(crate) fn heap_bytes(pixel_count: usize) -> usize {
        BitBuffer2::heap_bytes(Size2us::new(pixel_count, 1))
    }

    /// Intersect every frame's coverage into the pixels all of them contribute at.
    ///
    /// In parallel over spans of the mask, each reading its span of every frame: AND is
    /// order-independent, so the mask is the serial one, and only one mask is ever held.
    pub(super) fn build(
        coverage: ChannelCoverage<'_>,
        cancel: &CancelToken,
    ) -> Result<Self, StackError> {
        let mut common_domain = Self::full_mask(coverage.pixel_count);
        common_domain
            .words
            .par_chunks_mut(Cancelled::POLL_CHUNK / WORD_BITS)
            .enumerate()
            .try_for_each(|(span, words)| coverage.intersect(span, words, cancel))?;
        Self::of_mask(common_domain)
    }

    /// [`Self::build`] one span after another, for a caller already parallel over its jobs: a job
    /// that waited on a nested parallel loop would let its worker take up another job meanwhile,
    /// and the jobs alive at once, each holding its scratch, would outnumber the workers.
    pub(super) fn build_in_job(
        coverage: ChannelCoverage<'_>,
        cancel: &CancelToken,
    ) -> Result<Self, StackError> {
        let mut common_domain = Self::full_mask(coverage.pixel_count);
        for (span, words) in common_domain
            .words
            .chunks_mut(Cancelled::POLL_CHUNK / WORD_BITS)
            .enumerate()
        {
            coverage.intersect(span, words, cancel)?;
        }
        Self::of_mask(common_domain)
    }

    /// The domain `valid` holds; [`StackError::NoCommonCoverage`] when it holds no pixel.
    fn of_mask(valid: BitBuffer2) -> Result<Self, StackError> {
        debug_assert!(
            valid.padding_is_clear(),
            "a domain's mask past its last pixel"
        );
        let sample_count = valid.count_ones();
        if sample_count == 0 {
            return Err(StackError::NoCommonCoverage);
        }
        Ok(Self {
            valid,
            sample_count,
        })
    }

    /// The pixels of this domain over an image of `size` at whose position `keeps` holds: the
    /// photosites of one colour of a mosaic, for one.
    ///
    /// # Errors
    /// [`StackError::NoCommonCoverage`] when it keeps none of them.
    pub(super) fn restricted(
        &self,
        size: Size2us,
        keeps: impl Fn(Vec2us) -> bool + Sync,
        cancel: &CancelToken,
    ) -> Result<Self, StackError> {
        let pixel_count = size.pixel_count();
        let mut valid = self.valid.clone();
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
                        keeps(Vec2us::new(pixel % size.width, pixel / size.width))
                    });
                }
                Ok::<_, StackError>(())
            })?;
        Self::of_mask(valid)
    }

    /// Call `visit` with the values of `values` the domain holds, in pixel order: a mask word's 64
    /// values as one run when it holds them all, the others one value a run.
    pub(super) fn for_each_in(
        &self,
        values: &[f32],
        cancel: &CancelToken,
        mut visit: impl FnMut(&[f32]),
    ) -> Result<(), StackError> {
        for (chunk, chunk_values) in values.chunks(Cancelled::POLL_CHUNK).enumerate() {
            Cancelled::check(cancel)?;
            let base = chunk * Cancelled::POLL_CHUNK;
            let end = base + chunk_values.len();
            // The mask is one row, so word `w` covers pixels `64w..64w + 64`, and a chunk starts on
            // a word: its words are read once each instead of a bit lookup per pixel, and a full
            // word's pixels go on as one run.
            let words = &self.valid.words[base / WORD_BITS..end.div_ceil(WORD_BITS)];
            for (offset, &word) in words.iter().enumerate() {
                let word_base = offset * WORD_BITS;
                if word == u64::MAX {
                    visit(&chunk_values[word_base..word_base + WORD_BITS]);
                    continue;
                }
                let mut bits = word;
                while bits != 0 {
                    let bit = bits.trailing_zeros() as usize;
                    bits &= bits - 1;
                    visit(slice::from_ref(&chunk_values[word_base + bit]));
                }
            }
        }
        Ok(())
    }

    /// Up to [`SAMPLE_LIMIT`] pixel indices, ascending and evenly spread by rank over the pixels
    /// the domain holds, as [`StratifiedSamples`] spreads them over every pixel: the `k`-th of `m`
    /// is the one of rank `⌊k·n/m⌋` among the `n`.
    ///
    /// [`StratifiedSamples`]: crate::frame_store::stratified_samples::StratifiedSamples
    pub(super) fn stratified_indices(
        &self,
        cancel: &CancelToken,
    ) -> Result<Vec<usize>, StackError> {
        let sample_count = self.sample_count;
        let retained = sample_count.min(SAMPLE_LIMIT);
        let mut indices = Vec::with_capacity(retained);
        let mut rank = 0;
        // The mask is one row, so word `w` covers pixels `64w..64w + 64`; the cancel poll runs per
        // word group rather than per pixel.
        for (group, words) in self
            .valid
            .words
            .chunks(Cancelled::POLL_CHUNK / WORD_BITS)
            .enumerate()
        {
            Cancelled::check(cancel)?;
            for (offset, &word) in words.iter().enumerate() {
                let base = (group * (Cancelled::POLL_CHUNK / WORD_BITS) + offset) * WORD_BITS;
                let mut bits = word;
                while bits != 0 && indices.len() < retained {
                    let bit = bits.trailing_zeros() as usize;
                    bits &= bits - 1;
                    if rank == indices.len() * sample_count / retained {
                        indices.push(base + bit);
                    }
                    rank += 1;
                }
            }
        }
        debug_assert_eq!(indices.len(), retained);
        Ok(indices)
    }
}

/// Every frame's coverage of one channel of a plane of `pixel_count` pixels: what a
/// [`CommonDomain`] intersects.
#[derive(Debug, Clone, Copy)]
pub(super) struct ChannelCoverage<'a> {
    pub(super) frames: &'a [&'a StoredFrame],
    pub(super) channel: usize,
    pub(super) pixel_count: usize,
}

impl ChannelCoverage<'_> {
    /// Intersect span `span` of a mask, its `words`, with every frame's coverage there.
    fn intersect(
        &self,
        span: usize,
        words: &mut [u64],
        cancel: &CancelToken,
    ) -> Result<(), StackError> {
        Cancelled::check(cancel)?;
        let base = span * Cancelled::POLL_CHUNK;
        if base < self.pixel_count {
            let end = (base + words.len() * WORD_BITS).min(self.pixel_count);
            for frame in self.frames {
                let gate = FrameGate::of(frame, self.channel, base..end);
                if !gate.everywhere() {
                    intersect_span(words, end - base, |index| gate.sample(index).is_some());
                }
            }
        }
        Ok(())
    }
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

    /// The common-domain mask is one bit per pixel, not one byte, [`CommonDomain::heap_bytes`] is
    /// what it holds, and its padding starts clear.
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

        // 1000 pixels end 40 bits into their last word, and the 24 bits past them are clear, so a
        // walk over the words reads no pixel past the plane.
        let mask = CommonDomain::full_mask(1000);
        assert!(mask.padding_is_clear());
        assert_eq!(mask.count_ones(), 1000);
    }
}
