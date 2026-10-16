//! The bit masks one detection accumulates, for the mono detector and the Bayer phases it serves.

use rayon::prelude::*;

use crate::bit_buffer2::BitBuffer2;
use crate::calibration::cosmic_ray::FINE_STRUCTURE_SIGMA_FLOOR;
use crate::calibration::cosmic_ray::cosmic_ray_config::CosmicRayConfig;
use crate::math::size2us::Size2us;

/// Masks a detection holds: the accumulated one, plus `primary` and `flags`.
pub(crate) const CONCURRENT_MASKS: usize = 3;

/// The three cosmic-ray masks a detection holds, one bit per pixel each.
///
/// All three live as long as the detector and are cleared for each plane rather than rebuilt per
/// iteration or plane, which costs nothing at the peak — [`detect_and_grow`](Self::detect_and_grow)
/// needed all three live at once anyway, and a Bayer mosaic's largest phase runs first — and saves
/// two frame-sized allocations per pass and three per plane.
#[derive(Debug)]
pub(super) struct CrMasks {
    /// Every CR pixel found so far: the in-painting mask, and the count the detector returns.
    pub(super) accumulated: BitBuffer2,
    /// Pixels clearing the full `detection_sigma` and the contrast this iteration, before growth.
    primary: BitBuffer2,
    /// The first growth of `primary`, from which the second grows back into `primary`, which
    /// then merges into `accumulated`.
    flags: BitBuffer2,
}

impl CrMasks {
    /// The bytes the masks of a `size` detection hold.
    pub(super) const fn heap_bytes(size: Size2us) -> usize {
        CONCURRENT_MASKS * BitBuffer2::heap_bytes(size)
    }

    /// Masks with no pixels yet, for [`Self::clear_to`] to size.
    pub(super) const fn empty() -> Self {
        Self {
            accumulated: BitBuffer2::empty(),
            primary: BitBuffer2::empty(),
            flags: BitBuffer2::empty(),
        }
    }

    /// All three clear at `size`, in the allocations a previous detection left.
    pub(super) fn clear_to(&mut self, size: Size2us) {
        for mask in [&mut self.accumulated, &mut self.primary, &mut self.flags] {
            mask.clear_to(size);
        }
    }

    /// Flag CRs, as astroscrappy does: `S' > detection_sigma` **and** the fine-structure contrast
    /// `S' > contrast_limit·(F/noise)`; then grow them by the 3×3 box, keeping the pixels where
    /// `S' > detection_sigma`, and grow that by the box again, keeping those where
    /// `S' > detection_sigma·growth_fraction`. The growth takes no contrast test: a hit's wings
    /// carry its light, not a star's. Merges the result into `accumulated` and returns how many
    /// pixels that added — zero ends the detect→replace loop.
    ///
    /// The contrast is van Dokkum's `L⁺/F > contrast_limit` written in astroscrappy's
    /// noise-normalized form: comparing the significance image `S'` against
    /// `contrast_limit·(F/noise)` (rather than raw `L⁺` against `contrast_limit·F`) puts `F` in the
    /// same units as `S'`, so the `contrast_limit` default carries the same star-core protection as
    /// astroscrappy/ccdproc. (Raw `L⁺ > contrast_limit·F` is ~2× more aggressive.)
    ///
    /// No pixel `never_flagged` holds is a candidate or grown into, as astroscrappy keeps its
    /// saturated stars and bad pixels out with `goodpix`.
    pub(super) fn detect_and_grow(
        &mut self,
        planes: DetectionPlanes<'_>,
        cfg: &CosmicRayConfig,
        never_flagged: Option<&BitBuffer2>,
    ) -> usize {
        let DetectionPlanes {
            significance,
            fine: f,
            noise,
        } = planes;
        let Self {
            accumulated,
            primary,
            flags,
        } = self;
        primary.fill_from_predicate(|i| {
            let f_norm = (f[i] / noise[i]).max(FINE_STRUCTURE_SIGMA_FLOOR);
            significance[i] > cfg.detection_sigma && significance[i] > cfg.contrast_limit * f_norm
        });
        primary.and_not(accumulated);
        if let Some(never) = never_flagged {
            primary.and_not(never);
        }
        Growth {
            accumulated,
            never_flagged,
            keep: |i| significance[i] > cfg.detection_sigma,
        }
        .grow_box(primary, flags);
        let lowered = cfg.detection_sigma * cfg.growth_fraction;
        Growth {
            accumulated,
            never_flagged,
            keep: |i| significance[i] > lowered,
        }
        .grow_box(flags, primary);
        let flags = primary;

        // Word-wise: `flags & !accumulated` is what is newly set, then `accumulated |= flags`. The
        // padding is clear in both, so whole words count no pixel past a row.
        let mut newly = 0usize;
        for y in 0..accumulated.size.height {
            for (acc, &new) in accumulated
                .row_words_mut(y)
                .iter_mut()
                .zip(flags.row_words(y))
            {
                newly += (new & !*acc).count_ones() as usize;
                *acc |= new;
            }
        }
        newly
    }
}

/// What one pass of a detection decides by, a value per pixel each: the significance with its
/// smooth part taken out `S'`, the object fine structure `F`, and the noise `N`.
#[derive(Debug, Clone, Copy)]
pub(super) struct DetectionPlanes<'a> {
    pub(super) significance: &'a [f32],
    pub(super) fine: &'a [f32],
    pub(super) noise: &'a [f32],
}

/// The pixels a growth may take: those `keep` passes, by row-major index, that neither
/// `accumulated` nor `never_flagged` holds.
#[derive(Debug, Clone, Copy)]
struct Growth<'a, K> {
    accumulated: &'a BitBuffer2,
    never_flagged: Option<&'a BitBuffer2>,
    keep: K,
}

impl<K: Fn(usize) -> bool + Sync> Growth<'_, K> {
    /// Into `dest`, the pixels of the 3×3 box about any pixel of `source` the growth may take.
    ///
    /// Word by word: a row's three neighbouring source rows, each smeared one pixel either way
    /// across word boundaries, give the box; `keep` is asked only of the bits that survive it,
    /// which a sparse mask keeps few.
    fn grow_box(self, source: &BitBuffer2, dest: &mut BitBuffer2) {
        let Self {
            accumulated,
            never_flagged,
            keep,
        } = self;
        let Size2us { width, height } = source.size;
        let used_words = width.div_ceil(64);
        let last_word_mask = source.last_word_mask();
        dest.par_rows_mut().enumerate().for_each(|(y, out_row)| {
            out_row.fill(0);
            for (w, out) in out_row[..used_words].iter_mut().enumerate() {
                let mut boxed = 0u64;
                for source_y in y.saturating_sub(1)..=(y + 1).min(height - 1) {
                    let row = &source.row_words(source_y)[..used_words];
                    boxed |= row[w] | (row[w] << 1) | (row[w] >> 1);
                    if w > 0 {
                        boxed |= row[w - 1] >> 63;
                    }
                    if w + 1 < used_words {
                        boxed |= row[w + 1] << 63;
                    }
                }
                if w + 1 == used_words {
                    boxed &= last_word_mask;
                }
                let mut candidates = boxed & !accumulated.row_words(y)[w];
                if let Some(never) = never_flagged {
                    candidates &= !never.row_words(y)[w];
                }
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
}

#[cfg(test)]
pub(crate) mod internals {
    use crate::bit_buffer2::BitBuffer2;
    use crate::calibration::cosmic_ray::masks::CrMasks;
    use crate::math::size2us::Size2us;

    /// One of the masks a detection on a `size` plane holds, sized as the detector sizes it, for
    /// `cosmic_ray_scratch` to weigh.
    pub(crate) fn new_cr_mask(size: Size2us) -> BitBuffer2 {
        let mut masks = CrMasks::empty();
        masks.clear_to(size);
        masks.accumulated
    }
}
