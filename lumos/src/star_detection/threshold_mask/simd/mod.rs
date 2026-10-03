//! The packed threshold kernel: each `u64` word holds 64 pixels' comparisons.

use crate::simd::{F32_LANES, F32x8, Isa, Kernel, Mask8};
use crate::star_detection::threshold_mask::ThresholdParams;

/// Pixels per packed word.
const WORD_PIXELS: usize = 64;

/// Threshold one row into its packed words, on the widest Isa this CPU has.
///
/// With `WITH_BG` the level is `bg + σ·noise`; otherwise it is `σ·noise` (matched-filter case —
/// background already subtracted), and `bg` is unused and may be empty. Every word past the row's
/// pixels is cleared, which keeps the row padding of a `BitBuffer2` clear.
#[cfg_attr(not(test), inline)]
pub(super) fn process_words<const WITH_BG: bool>(
    pixels: &[f32],
    bg: &[f32],
    noise: &[f32],
    threshold: ThresholdParams,
    words: &mut [u64],
) {
    ProcessWords::<WITH_BG> {
        pixels,
        bg,
        noise,
        threshold,
        words,
    }
    .dispatch();
}

/// [`process_words`] as a kernel.
///
/// A packed word is a set of detections, so one differing bit is one pixel two paths disagree
/// about. The level is therefore the unfused `σ · max(noise, floor)` plus `bg` the scalar
/// reference computes: a fused multiply-add would round differently from it at `px == level`.
#[derive(Debug)]
struct ProcessWords<'a, const WITH_BG: bool> {
    pixels: &'a [f32],
    bg: &'a [f32],
    noise: &'a [f32],
    threshold: ThresholdParams,
    words: &'a mut [u64],
}

impl<const WITH_BG: bool> Kernel for ProcessWords<'_, WITH_BG> {
    type Output = ();

    #[inline(always)]
    fn run<S: Isa>(self, isa: S) {
        let len = self.pixels.len();
        debug_assert_eq!(self.noise.len(), len, "one noise sample per pixel");
        debug_assert!(
            !WITH_BG || self.bg.len() == len,
            "one background sample per pixel"
        );
        debug_assert!(
            self.words.len() >= len.div_ceil(WORD_PIXELS),
            "a word per 64 pixels"
        );

        let threshold = Levels {
            sigma: isa.splat_f32(self.threshold.sigma),
            min_noise: isa.splat_f32(self.threshold.min_noise),
        };
        let (pixel_words, pixel_tail) = self.pixels.as_chunks::<WORD_PIXELS>();
        let (noise_words, noise_tail) = self.noise.as_chunks::<WORD_PIXELS>();
        let (bg_words, bg_tail) = self.bg.as_chunks::<WORD_PIXELS>();
        let (full, rest) = self.words.split_at_mut(pixel_words.len());

        for (i, (word, (pixels, noise))) in full
            .iter_mut()
            .zip(pixel_words.iter().zip(noise_words))
            .enumerate()
        {
            let bg = if WITH_BG { &bg_words[i] } else { pixels };
            *word = threshold.word_bits::<S, WITH_BG>(isa, pixels, bg, noise);
        }

        let Some((last, padding)) = rest.split_first_mut() else {
            return;
        };
        padding.fill(0);
        *last = if pixel_tail.is_empty() {
            0
        } else {
            let bg = if WITH_BG { bg_tail } else { pixel_tail };
            let bits = threshold.word_bits::<S, WITH_BG>(
                isa,
                &padded(pixel_tail),
                &padded(bg),
                &padded(noise_tail),
            );
            bits & ((1 << pixel_tail.len()) - 1)
        };
    }
}

/// The threshold's two parameters, splat across the lanes.
#[derive(Debug, Clone, Copy)]
struct Levels<V> {
    sigma: V,
    min_noise: V,
}

impl<V: F32x8> Levels<V> {
    /// Bit `i` set where pixel `i` of the word exceeds its level.
    #[inline(always)]
    fn word_bits<S: Isa<F32 = V>, const WITH_BG: bool>(
        self,
        isa: S,
        pixels: &[f32; WORD_PIXELS],
        bg: &[f32; WORD_PIXELS],
        noise: &[f32; WORD_PIXELS],
    ) -> u64 {
        let (pixels, []) = pixels.as_chunks::<F32_LANES>() else {
            unreachable!("a word is whole vectors")
        };
        let (bg, []) = bg.as_chunks::<F32_LANES>() else {
            unreachable!("a word is whole vectors")
        };
        let (noise, []) = noise.as_chunks::<F32_LANES>() else {
            unreachable!("a word is whole vectors")
        };
        let mut bits = 0u64;
        for (group, ((pixels, bg), noise)) in pixels.iter().zip(bg).zip(noise).enumerate() {
            let level = self.sigma * isa.load_f32(noise).max(self.min_noise);
            let level = if WITH_BG {
                isa.load_f32(bg) + level
            } else {
                level
            };
            let above = isa.load_f32(pixels).lanes_gt(level).to_bitmask();
            bits |= u64::from(above) << (group * F32_LANES);
        }
        bits
    }
}

/// A partial word's samples, zero past them.
#[inline(always)]
fn padded(samples: &[f32]) -> [f32; WORD_PIXELS] {
    let mut word = [0.0; WORD_PIXELS];
    word[..samples.len()].copy_from_slice(samples);
    word
}

#[cfg(test)]
mod internals {
    use crate::star_detection::threshold_mask::ThresholdParams;
    use crate::star_detection::threshold_mask::simd::WORD_PIXELS;

    /// The scalar reference the kernel is tested and benched against, written apart from it.
    pub(super) fn process_words_scalar<const WITH_BG: bool>(
        pixels: &[f32],
        bg: &[f32],
        noise: &[f32],
        threshold: ThresholdParams,
        words: &mut [u64],
    ) {
        for (word_idx, word) in words.iter_mut().enumerate() {
            let mut bits = 0u64;
            for bit in 0..WORD_PIXELS {
                let px_idx = word_idx * WORD_PIXELS + bit;
                if px_idx >= pixels.len() {
                    break;
                }
                let mut level = threshold.level(noise[px_idx]);
                if WITH_BG {
                    level += bg[px_idx];
                }
                if pixels[px_idx] > level {
                    bits |= 1u64 << bit;
                }
            }
            *word = bits;
        }
    }
}

#[cfg(all(test, feature = "bench"))]
mod bench;
#[cfg(test)]
mod tests;
