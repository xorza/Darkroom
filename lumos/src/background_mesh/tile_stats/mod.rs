//! Per-tile SExtractor sky estimation: pixel collection (masked/sampled), sigma-clipped robust
//! statistics, and the crowding-aware Pearson-mode sky estimator for a single tile box.

use crate::bit_buffer2::BitBuffer2;
use crate::math::statistics::ClippedStats;
use crate::math::urect::URect;
use imaginarium::Buffer2;

/// Maximum samples per tile for statistics computation.
pub(crate) const MAX_TILE_SAMPLES: usize = 1024;

/// Tile statistics computed during background estimation.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct TileStats {
    /// Sky level: SExtractor's crowding-aware estimator (Pearson mode, median fallback when
    /// strongly skewed) over the sigma-clip survivors. Computed by [`TileStats::compute`].
    pub(crate) sky: f32,
    pub(crate) sigma: f32,
}

/// Which of a tile's two statistics a spline pass is working on.
///
/// The sky and sigma planes are interpolated by identical code over identical grids; naming the
/// plane rather than passing a `fn(&TileStats) -> f32` is what keeps that one loop instead of two.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TileComponent {
    Sky,
    Sigma,
}

impl TileComponent {
    /// Both components, in the order the spline solver visits them.
    pub(crate) const ALL: [Self; 2] = [Self::Sky, Self::Sigma];
}

/// The second derivative in Y of each tile statistic, for the natural cubic spline. Paired so a
/// pass cannot compute one plane's derivative and forget the other's.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct TileD2y {
    pub(crate) sky: f32,
    pub(crate) sigma: f32,
}

impl TileD2y {
    pub(crate) fn get(self, component: TileComponent) -> f32 {
        match component {
            TileComponent::Sky => self.sky,
            TileComponent::Sigma => self.sigma,
        }
    }

    pub(crate) fn get_mut(&mut self, component: TileComponent) -> &mut f32 {
        match component {
            TileComponent::Sky => &mut self.sky,
            TileComponent::Sigma => &mut self.sigma,
        }
    }
}

impl TileStats {
    /// The statistic `component` names.
    pub(crate) fn get(self, component: TileComponent) -> f32 {
        match component {
            TileComponent::Sky => self.sky,
            TileComponent::Sigma => self.sigma,
        }
    }

    /// Compute sigma-clipped statistics for the pixels of `tile`.
    ///
    /// When a mask is provided, only unmasked pixels are used. If all pixels
    /// are masked, falls back to sampling all pixels (including masked) as a
    /// last resort. A noisy estimate from few background pixels is far better
    /// than a biased estimate contaminated by star flux.
    pub(crate) fn compute(
        pixels: &Buffer2<f32>,
        mask: Option<&BitBuffer2>,
        tile: URect,
        sigma_clip_iterations: usize,
        values: &mut Vec<f32>,
        deviations: &mut Vec<f32>,
    ) -> Self {
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
            return Self::default();
        }

        let stats = ClippedStats::sigma_clipped(values, deviations, 3.0, sigma_clip_iterations);

        Self {
            sky: sextractor_sky(&stats),
            sigma: stats.sigma,
        }
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
mod tests;
