//! The bit masks one detection accumulates, shared by the mono and X-Trans paths.

use crate::bit_buffer2::BitBuffer2;
use crate::math::size2us::Size2us;
use crate::math::vec2us::Vec2us;

use crate::stacking::calibration_masters::cosmic_ray::FINE_STRUCTURE_SIGMA_FLOOR;
use crate::stacking::calibration_masters::cosmic_ray::config::CosmicRayConfig;

/// The three cosmic-ray masks a detection holds, one bit per pixel each.
///
/// All three live for the whole detection rather than being rebuilt per iteration, which costs
/// nothing at the peak — [`detect_and_grow`](Self::detect_and_grow) needed all three live at once
/// anyway — and saves two frame-sized allocations per pass.
#[derive(Debug)]
pub(super) struct CrMasks {
    /// Every CR pixel found so far: the in-painting mask, and the count the detector returns.
    pub(super) accumulated: BitBuffer2,
    /// Pixels clearing the full `sigclip` this iteration, before growth.
    primary: BitBuffer2,
    /// `primary` plus the wings grown onto it — what merges into `accumulated`.
    flags: BitBuffer2,
}

impl CrMasks {
    pub(super) fn new(size: Size2us) -> Self {
        Self {
            accumulated: new_cr_mask(size),
            primary: new_cr_mask(size),
            flags: new_cr_mask(size),
        }
    }

    /// Flag CRs: `S' > sigclip` **and** the fine-structure contrast `S' > objlim·(F/noise)`, then
    /// grow onto neighbors clearing the lowered threshold `sigclip·sigfrac` and the same contrast
    /// test (a flagged CR's fainter wings). Merges the result into `accumulated` and returns how
    /// many pixels that added — zero ends the detect→replace loop.
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
        let size = accumulated.size;
        let passes_contrast = |i: usize, sig_thresh: f32| {
            let f_norm = (f[i] / noise[i]).max(FINE_STRUCTURE_SIGMA_FLOOR);
            significance[i] > sig_thresh && significance[i] > cfg.objlim * f_norm
        };
        primary.fill_from_predicate(|i| !accumulated.get(i) && passes_contrast(i, cfg.sigclip));

        let lowered = cfg.sigclip * cfg.sigfrac;
        flags.copy_from(primary);
        for y in 0..size.height {
            for x in 0..size.width {
                if !primary.get_at(Vec2us::new(x, y)) {
                    continue;
                }
                let y0 = y.saturating_sub(1);
                let y1 = (y + 1).min(size.height - 1);
                let x0 = x.saturating_sub(1);
                let x1 = (x + 1).min(size.width - 1);
                for ny in y0..=y1 {
                    for nx in x0..=x1 {
                        let j = size.index_of(Vec2us::new(nx, ny));
                        if !flags.get(j) && !accumulated.get(j) && passes_contrast(j, lowered) {
                            flags.set(j, true);
                        }
                    }
                }
            }
        }

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

    /// Masks a detection holds: the accumulated one, plus `primary` and `flags`.
    pub(crate) const CONCURRENT_MASKS: usize = 3;

    /// The mask as the detector allocates it, for `mem_budget` to weigh.
    pub(crate) fn new_cr_mask(size: Size2us) -> BitBuffer2 {
        super::new_cr_mask(size)
    }
}
