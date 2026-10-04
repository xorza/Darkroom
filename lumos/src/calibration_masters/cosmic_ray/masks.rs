//! The bit masks one detection accumulates, shared by the mono and X-Trans paths.

use rayon::prelude::*;

use crate::bit_buffer2::BitBuffer2;
use crate::math::size2us::Size2us;

use crate::calibration_masters::cosmic_ray::FINE_STRUCTURE_SIGMA_FLOOR;
use crate::calibration_masters::cosmic_ray::config::CosmicRayConfig;

/// Masks a detection holds: the accumulated one, plus `primary` and `flags`.
pub(crate) const CONCURRENT_MASKS: usize = 3;

/// The three cosmic-ray masks a detection holds, one bit per pixel each.
///
/// All three live for the whole detection rather than being rebuilt per iteration, which costs
/// nothing at the peak — [`detect_and_grow`](Self::detect_and_grow) needed all three live at once
/// anyway — and saves two frame-sized allocations per pass.
#[derive(Debug)]
pub(super) struct CrMasks {
    /// Every CR pixel found so far: the in-painting mask, and the count the detector returns.
    pub(super) accumulated: BitBuffer2,
    /// Pixels clearing the full `sigclip` and the contrast this iteration, before growth.
    primary: BitBuffer2,
    /// The first growth of `primary`, from which the second grows back into `primary`, which
    /// then merges into `accumulated`.
    flags: BitBuffer2,
}

impl CrMasks {
    /// The bytes the masks of a `size` detection hold.
    pub(super) fn heap_bytes(size: Size2us) -> usize {
        CONCURRENT_MASKS * BitBuffer2::heap_bytes(size)
    }

    pub(super) fn new(size: Size2us) -> Self {
        Self {
            accumulated: new_cr_mask(size),
            primary: new_cr_mask(size),
            flags: new_cr_mask(size),
        }
    }

    /// Flag CRs, as astroscrappy does: `S' > sigclip` **and** the fine-structure contrast
    /// `S' > objlim·(F/noise)`; then grow them by the 3×3 box, keeping the pixels where
    /// `S' > sigclip`, and grow that by the box again, keeping those where `S' > sigclip·sigfrac`.
    /// The growth takes no contrast test: a hit's wings carry its light, not a star's. Merges the
    /// result into `accumulated` and returns how many pixels that added — zero ends the
    /// detect→replace loop.
    ///
    /// The contrast is van Dokkum's `L⁺/F > objlim` written in astroscrappy's noise-normalized
    /// form: comparing the significance image `S'` against `objlim·(F/noise)` (rather than raw `L⁺`
    /// against `objlim·F`) puts `F` in the same units as `S'`, so the `objlim` default carries the
    /// same star-core protection as astroscrappy/ccdproc. (Raw `L⁺ > objlim·F` is ~2× more
    /// aggressive.)
    pub(super) fn detect_and_grow(
        &mut self,
        significance: &[f32],
        f: &[f32],
        noise: &[f32],
        cfg: &CosmicRayConfig,
    ) -> usize {
        let Self {
            accumulated,
            primary,
            flags,
        } = self;
        primary.fill_from_predicate(|i| {
            let f_norm = (f[i] / noise[i]).max(FINE_STRUCTURE_SIGMA_FLOOR);
            significance[i] > cfg.sigclip && significance[i] > cfg.objlim * f_norm
        });
        primary.and_not(accumulated);
        grow_box(primary, flags, accumulated, |i| {
            significance[i] > cfg.sigclip
        });
        let lowered = cfg.sigclip * cfg.sigfrac;
        grow_box(flags, primary, accumulated, |i| significance[i] > lowered);
        let flags = primary;

        // Word-wise: `flags & !accumulated` is what is newly set, then `accumulated |= flags`.
        // Counting whole words needs no per-row masking only because both buffers have their
        // padding clear.
        debug_assert!(
            accumulated.padding_is_clear() && flags.padding_is_clear(),
            "padding bits would be counted as newly-flagged pixels"
        );
        let mut newly = 0usize;
        for (acc, &new) in accumulated.words.iter_mut().zip(&flags.words) {
            newly += (new & !*acc).count_ones() as usize;
            *acc |= new;
        }
        newly
    }
}

/// Into `dest`, the pixels of the 3×3 box about any pixel of `source` that `keep` passes, by
/// row-major index, and that `accumulated` does not hold.
///
/// Word by word: a row's three neighbouring source rows, each smeared one pixel either way across
/// word boundaries, give the box; `keep` is asked only of the bits that survive it, which a sparse
/// mask keeps few.
fn grow_box(
    source: &BitBuffer2,
    dest: &mut BitBuffer2,
    accumulated: &BitBuffer2,
    keep: impl Fn(usize) -> bool + Sync,
) {
    let Size2us { width, height } = source.size;
    let words_per_row = source.words_per_row();
    let used_words = width.div_ceil(64);
    let last_bits = width - used_words.saturating_sub(1) * 64;
    dest.words
        .par_chunks_mut(words_per_row.max(1))
        .enumerate()
        .for_each(|(y, out_row)| {
            out_row.fill(0);
            for (w, out) in out_row[..used_words].iter_mut().enumerate() {
                let mut boxed = 0u64;
                for source_y in y.saturating_sub(1)..=(y + 1).min(height - 1) {
                    let row = &source.words[source_y * words_per_row..][..used_words];
                    boxed |= row[w] | (row[w] << 1) | (row[w] >> 1);
                    if w > 0 {
                        boxed |= row[w - 1] >> 63;
                    }
                    if w + 1 < used_words {
                        boxed |= row[w + 1] << 63;
                    }
                }
                if w + 1 == used_words && last_bits < 64 {
                    boxed &= (1u64 << last_bits) - 1;
                }
                let mut candidates = boxed & !accumulated.words[y * words_per_row + w];
                while candidates != 0 {
                    let bit = candidates.trailing_zeros() as usize;
                    if keep(y * width + w * 64 + bit) {
                        *out |= 1 << bit;
                    }
                    candidates &= candidates - 1;
                }
            }
        });
}

/// One cosmic-ray mask: one bit per pixel.
///
/// Its own function so `mem_budget` can weigh exactly what the detector allocates. A detection
/// holds three ([`CrMasks`]), so the packing is worth three times its face value.
fn new_cr_mask(size: Size2us) -> BitBuffer2 {
    BitBuffer2::new_default(size)
}

#[cfg(test)]
pub(crate) mod internals {
    use crate::bit_buffer2::BitBuffer2;
    use crate::math::size2us::Size2us;

    /// The mask as the detector allocates it, for `mem_budget` to weigh.
    pub(crate) fn new_cr_mask(size: Size2us) -> BitBuffer2 {
        super::new_cr_mask(size)
    }
}
