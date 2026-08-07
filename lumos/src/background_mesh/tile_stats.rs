//! Per-tile SExtractor sky estimation: pixel collection (masked/sampled), sigma-clipped robust
//! statistics, and the crowding-aware Pearson-mode sky estimator for a single tile box.

use crate::background_mesh::TileStats;
use crate::bit_buffer2::BitBuffer2;
use crate::math::rect::URect;
use crate::math::statistics::ClippedStats;
use crate::math::statistics::sigma_clipped_median_mad;
use imaginarium::Buffer2;

/// Maximum samples per tile for statistics computation.
pub(crate) const MAX_TILE_SAMPLES: usize = 1024;

/// Compute sigma-clipped statistics for the pixels of `tile`.
///
/// When a mask is provided, only unmasked pixels are used. If all pixels
/// are masked, falls back to sampling all pixels (including masked) as a
/// last resort. A noisy estimate from few background pixels is far better
/// than a biased estimate contaminated by star flux.
pub(crate) fn compute_tile_stats(
    pixels: &Buffer2<f32>,
    mask: Option<&BitBuffer2>,
    tile: URect,
    sigma_clip_iterations: usize,
    values: &mut Vec<f32>,
    deviations: &mut Vec<f32>,
) -> TileStats {
    values.clear();

    match mask {
        Some(m) => {
            collect_unmasked_pixels(pixels, m, tile, values);
            if values.is_empty() {
                // All pixels masked — no choice but to use all pixels
                collect_sampled_pixels(pixels, tile, values);
            }
        }
        None => {
            if tile.area() <= MAX_TILE_SAMPLES {
                collect_all_pixels(pixels, tile, values);
            } else {
                collect_sampled_pixels(pixels, tile, values);
            }
        }
    }

    if values.is_empty() {
        return TileStats::default();
    }

    let stats = sigma_clipped_median_mad(values, deviations, 3.0, sigma_clip_iterations);

    TileStats {
        sky: sextractor_sky(&stats),
        sigma: stats.sigma,
    }
}

/// SExtractor's crowding-aware sky estimator (Bertin & Arnouts 1996, `back.c`).
///
/// Even after clipping, the sky histogram keeps a bright-ward tail from faint sources, so
/// `mean > median > mode` and the median alone systematically over-estimates the sky. Pearson's
/// empirical mode `2.5·median − 1.5·mean` cancels that residual skew. When the tile is strongly
/// skewed (crowded: `|mean − median| ≥ 0.3·σ`) the extrapolation becomes unreliable, so it falls
/// back to the plain median. σ = 0 also takes the fallback — the clip couldn't separate outliers
/// there (zero spread estimate), so the mean is untrustworthy while the median stays robust.
fn sextractor_sky(stats: &ClippedStats) -> f32 {
    if (stats.mean - stats.median).abs() < 0.3 * stats.sigma {
        2.5 * stats.median - 1.5 * stats.mean
    } else {
        stats.median
    }
}

/// Collect all pixels from a tile region.
#[inline]
fn collect_all_pixels(pixels: &Buffer2<f32>, tile: URect, values: &mut Vec<f32>) {
    let width = pixels.width();
    let tile_width = tile.width();
    for y in tile.min.y..tile.max.y {
        let row_start = y * width + tile.min.x;
        values.extend_from_slice(&pixels[row_start..row_start + tile_width]);
    }
}

/// Collect sampled pixels using strided access (~MAX_TILE_SAMPLES pixels).
#[inline]
fn collect_sampled_pixels(pixels: &Buffer2<f32>, tile: URect, values: &mut Vec<f32>) {
    let width = pixels.width();
    let stride = ((tile.area() as f32 / MAX_TILE_SAMPLES as f32).max(1.0))
        .sqrt()
        .ceil() as usize;

    for y in (tile.min.y..tile.max.y).step_by(stride) {
        let row_start = y * width;
        for x in (tile.min.x..tile.max.x).step_by(stride) {
            values.push(pixels[row_start + x]);
        }
    }
}

#[inline]
fn collect_unmasked_pixels(
    pixels: &Buffer2<f32>,
    mask: &BitBuffer2,
    tile: URect,
    values: &mut Vec<f32>,
) {
    let unmasked_count = count_unmasked_pixels(mask, tile);
    let sample_count = unmasked_count.min(MAX_TILE_SAMPLES);
    if sample_count == 0 {
        return;
    }

    let width = pixels.width();
    let mask_words = &mask.words;
    let words_per_row = mask.words_per_row();
    let ordinal_step = unmasked_count / sample_count;
    let ordinal_remainder = unmasked_count % sample_count;
    let mut next_ordinal = 0;
    let mut remainder_accumulator = 0;
    let mut ordinal = 0;
    let mut selected_count = 0;

    for y in tile.min.y..tile.max.y {
        let row_start = y * width;
        let word_row_start = y * words_per_row;
        let mut x = tile.min.x;

        while x < tile.max.x {
            let word_idx = x / 64;
            let bit_offset = x % 64;
            let mask_word = mask_words[word_row_start + word_idx];
            let bits_to_process = (64 - bit_offset).min(tile.max.x - x);
            let mut bits = unmasked_bits(mask_word, bit_offset, bits_to_process);
            while bits != 0 {
                if ordinal == next_ordinal {
                    values.push(pixels[row_start + x + bits.trailing_zeros() as usize]);
                    selected_count += 1;
                    if selected_count == sample_count {
                        return;
                    }
                    next_ordinal += ordinal_step;
                    remainder_accumulator += ordinal_remainder;
                    if remainder_accumulator >= sample_count {
                        next_ordinal += 1;
                        remainder_accumulator -= sample_count;
                    }
                }
                ordinal += 1;
                bits &= bits - 1;
            }
            x += bits_to_process;
        }
    }
    unreachable!("unmasked pixel count changed between sampling passes");
}

#[inline]
fn count_unmasked_pixels(mask: &BitBuffer2, tile: URect) -> usize {
    let mask_words = &mask.words;
    let words_per_row = mask.words_per_row();
    let mut count = 0;

    for y in tile.min.y..tile.max.y {
        let word_row_start = y * words_per_row;
        let mut x = tile.min.x;
        while x < tile.max.x {
            let word_idx = x / 64;
            let bit_offset = x % 64;
            let bits_to_process = (64 - bit_offset).min(tile.max.x - x);
            count += unmasked_bits(
                mask_words[word_row_start + word_idx],
                bit_offset,
                bits_to_process,
            )
            .count_ones() as usize;
            x += bits_to_process;
        }
    }

    count
}

#[inline]
fn unmasked_bits(mask_word: u64, bit_offset: usize, bits_to_process: usize) -> u64 {
    let relevant_bits = if bits_to_process == 64 {
        !0
    } else {
        ((1u64 << bits_to_process) - 1) << bit_offset
    };
    (!mask_word & relevant_bits) >> bit_offset
}

#[cfg(test)]
fn reference_subsample(values: &mut Vec<f32>, target_size: usize) {
    let len = values.len();
    if len > target_size {
        for write_index in 0..target_size {
            values[write_index] = values[write_index * len / target_size];
        }
        values.truncate(target_size);
    }
}

#[cfg(test)]
mod tests {
    use crate::background_mesh::tile_stats::*;
    use crate::math::vec2us::Vec2us;

    #[test]
    fn sextractor_sky_hand_computed() {
        let stats = |median: f32, mean: f32, sigma: f32| ClippedStats {
            median,
            sigma,
            mean,
        };
        // Mild skew (|mean−median| = 0.2 < 0.3σ): Pearson mode 2.5·100 − 1.5·100.2 = 99.7,
        // pulled below the median toward the histogram peak.
        let sky = sextractor_sky(&stats(100.0, 100.2, 1.0));
        assert!((sky - 99.7).abs() < 1e-4, "mode = 99.7, got {sky}");
        // Strong skew (1.0 ≥ 0.3σ): the mode extrapolation is unreliable → plain median.
        assert_eq!(sextractor_sky(&stats(100.0, 101.0, 1.0)), 100.0);
        // Symmetric histogram: mode = 2.5·m − 1.5·m = m — estimator changes nothing.
        assert_eq!(sextractor_sky(&stats(100.0, 100.0, 1.0)), 100.0);
        // Uniform tile (σ = 0): |0| < 0 is false → median fallback.
        assert_eq!(sextractor_sky(&stats(5.0, 5.0, 0.0)), 5.0);
    }

    #[test]
    fn test_collect_sampled_pixels_small_tile() {
        let pixels = Buffer2::new_filled(32, 32, 0.5);
        let mut values = Vec::new();
        collect_sampled_pixels(
            &pixels,
            URect::new(Vec2us::ZERO, Vec2us::new(32, 32)),
            &mut values,
        );
        // Small tile should collect all or most pixels
        assert!(values.len() >= 100);
        assert!(values.iter().all(|&v| (v - 0.5).abs() < 0.01));
    }

    #[test]
    fn test_collect_sampled_pixels_large_tile() {
        let pixels = Buffer2::new_filled(256, 256, 0.5);
        let mut values = Vec::new();
        collect_sampled_pixels(
            &pixels,
            URect::new(Vec2us::ZERO, Vec2us::new(256, 256)),
            &mut values,
        );
        assert!(values.len() <= MAX_TILE_SAMPLES);
        assert!(values.iter().all(|&v| (v - 0.5).abs() < 0.01));
    }

    #[test]
    fn test_collect_unmasked_pixels_none_masked() {
        let pixels = Buffer2::new_filled(64, 64, 0.5);
        let mask = BitBuffer2::new_filled(64, 64, false);
        let mut values = Vec::new();
        collect_unmasked_pixels(
            &pixels,
            &mask,
            URect::new(Vec2us::ZERO, Vec2us::new(64, 64)),
            &mut values,
        );
        assert_eq!(values.len(), MAX_TILE_SAMPLES);
    }

    #[test]
    fn test_collect_unmasked_pixels_all_masked() {
        let pixels = Buffer2::new_filled(64, 64, 0.5);
        let mask = BitBuffer2::new_filled(64, 64, true);
        let mut values = Vec::new();
        collect_unmasked_pixels(
            &pixels,
            &mask,
            URect::new(Vec2us::ZERO, Vec2us::new(64, 64)),
            &mut values,
        );
        assert!(values.is_empty());
    }

    #[test]
    fn test_collect_unmasked_pixels_partial_mask() {
        let width = 64;
        let height = 64;
        let pixels = Buffer2::new_filled(width, height, 0.5);

        // Mask every other pixel
        let mut mask = BitBuffer2::new_filled(width, height, false);
        for y in 0..height {
            for x in 0..width {
                if (x + y) % 2 == 0 {
                    mask.set_xy(x, y, true);
                }
            }
        }

        let mut values = Vec::new();
        collect_unmasked_pixels(
            &pixels,
            &mask,
            URect::new(Vec2us::ZERO, Vec2us::new(64, 64)),
            &mut values,
        );
        assert_eq!(values.len(), MAX_TILE_SAMPLES);
    }

    #[test]
    fn test_collect_unmasked_pixels_partial_tile() {
        let pixels = Buffer2::new_filled(100, 100, 0.5);
        let mask = BitBuffer2::new_filled(100, 100, false);
        let mut values = Vec::new();
        collect_unmasked_pixels(
            &pixels,
            &mask,
            URect::new(Vec2us::new(10, 20), Vec2us::new(70, 80)),
            &mut values,
        );
        assert_eq!(values.len(), MAX_TILE_SAMPLES);
    }

    #[test]
    fn masked_sampling_matches_evenly_spaced_unmasked_ordinals() {
        #[derive(Debug)]
        struct SamplingCase {
            dimensions: (usize, usize),
            tile: URect,
            mask_modulus: Option<usize>,
        }

        let cases = [
            SamplingCase {
                dimensions: (17, 19),
                tile: URect::new(Vec2us::ZERO, Vec2us::new(17, 19)),
                mask_modulus: None,
            },
            SamplingCase {
                dimensions: (32, 32),
                tile: URect::new(Vec2us::ZERO, Vec2us::new(32, 32)),
                mask_modulus: None,
            },
            SamplingCase {
                dimensions: (40, 40),
                tile: URect::new(Vec2us::ZERO, Vec2us::new(40, 40)),
                mask_modulus: Some(7),
            },
            SamplingCase {
                dimensions: (130, 75),
                tile: URect::new(Vec2us::new(3, 2), Vec2us::new(129, 74)),
                mask_modulus: Some(5),
            },
            SamplingCase {
                dimensions: (256, 256),
                tile: URect::new(Vec2us::ZERO, Vec2us::new(256, 256)),
                mask_modulus: None,
            },
        ];

        for case in cases {
            let (width, height) = case.dimensions;
            let pixels = Buffer2::new(
                width,
                height,
                (0..width * height).map(|i| i as f32).collect(),
            );
            let mut mask = BitBuffer2::new_filled(width, height, false);
            if let Some(modulus) = case.mask_modulus {
                for y in 0..height {
                    for x in 0..width {
                        if (x + 3 * y) % modulus == 0 {
                            mask.set_xy(x, y, true);
                        }
                    }
                }
            }

            let mut expected: Vec<f32> = (case.tile.min.y..case.tile.max.y)
                .flat_map(|y| {
                    let mask = &mask;
                    let pixels = &pixels;
                    (case.tile.min.x..case.tile.max.x)
                        .filter(move |&x| !mask.get_xy(x, y))
                        .map(move |x| pixels[y * width + x])
                })
                .collect();
            reference_subsample(&mut expected, MAX_TILE_SAMPLES);

            let mut actual = Vec::new();
            collect_unmasked_pixels(&pixels, &mask, case.tile, &mut actual);

            assert_eq!(actual, expected, "case: {case:?}");
            assert!(
                actual.capacity() <= MAX_TILE_SAMPLES,
                "case retained {} samples: {case:?}",
                actual.capacity()
            );
        }
    }
}
