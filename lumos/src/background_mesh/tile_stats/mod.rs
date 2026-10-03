//! Per-tile SExtractor sky estimation: pixel collection (masked/sampled), sigma-clipped robust
//! statistics, read about the tile's own plane for the sky, and the crowding-aware Pearson-mode
//! sky estimator for a single tile box.

use crate::background_mesh::workspace::TileScratch;
use crate::bit_buffer2::BitBuffer2;
use crate::math::statistics::ClippedStats;
use crate::math::urect::URect;
use imaginarium::Buffer2;

/// Maximum samples per tile for statistics computation.
pub(crate) const MAX_TILE_SAMPLES: usize = 1024;
const _: () = assert!(
    MAX_TILE_SAMPLES.is_multiple_of(2),
    "sample_ordinal needs an even count"
);

/// The clip width, in σ, of the tile statistics and of the plane fit's survivors.
const CLIP_KAPPA: f32 = 3.0;

/// How many times the tile's plane is refitted to the survivors of the statistics about the last
/// one. The first fit reads survivors of a clip about a flat sky, whose σ the gradient still
/// inflates, so faint star wings inside that wide band tilt it; the second reads the clip about
/// the first plane, at the noise's own width. A third measured no better.
const PLANE_FITS: usize = 2;

/// Tile statistics computed during background estimation.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct TileStats {
    /// Sky level at the tile's centre: SExtractor's crowding-aware estimator (Pearson mode, median
    /// fallback when strongly skewed) over the survivors of a clip about the tile's own plane.
    /// Computed by [`TileStats::compute`].
    pub(crate) sky: f32,
    /// SExtractor's background RMS: the spread of the clip survivors of the raw samples, which
    /// counts the sky's own change across the tile as well as the noise.
    pub(crate) sigma: f32,
}

/// The slope of a plane over one tile, per unit of a doubled, centred offset (see [`centred`]).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct TileSlope {
    x: f64,
    y: f64,
}

impl TileSlope {
    /// The plane's rise from the tile's centre to the sample at `offset`.
    fn at(self, offset: [i32; 2]) -> f64 {
        self.x * f64::from(offset[0]) + self.y * f64::from(offset[1])
    }

    /// The least-squares slope of the samples whose value off this plane lies within `band` of
    /// `center`. `None` when those samples do not span a plane: fewer than three, or all on one
    /// line.
    ///
    /// The offsets are integers, so the position moments and the determinant are exact in `i128`,
    /// and "on one line" is an exact zero rather than a tolerance.
    fn refit(self, values: &[f32], offsets: &[[i32; 2]], center: f32, band: f32) -> Option<Self> {
        let (mut n, mut sx, mut sy, mut sxx, mut syy, mut sxy) =
            (0i128, 0i128, 0i128, 0i128, 0i128, 0i128);
        let (mut sv, mut sxv, mut syv) = (0.0f64, 0.0f64, 0.0f64);
        for (&value, &offset) in values.iter().zip(offsets) {
            let value = f64::from(value);
            if (value - self.at(offset) - f64::from(center)).abs() > f64::from(band) {
                continue;
            }
            let (x, y) = (i128::from(offset[0]), i128::from(offset[1]));
            n += 1;
            sx += x;
            sy += y;
            sxx += x * x;
            syy += y * y;
            sxy += x * y;
            sv += value;
            sxv += x as f64 * value;
            syv += y as f64 * value;
        }
        // The centred normal equations, each scaled by n: `[cxx cxy; cxy cyy]·slope = [cxv; cyv]`.
        let cxx = n * sxx - sx * sx;
        let cyy = n * syy - sy * sy;
        let cxy = n * sxy - sx * sy;
        let det = cxx * cyy - cxy * cxy;
        if n < 3 || det == 0 {
            return None;
        }
        let (n, sx, sy) = (n as f64, sx as f64, sy as f64);
        let cxv = n * sxv - sx * sv;
        let cyv = n * syv - sy * sv;
        let (cxx, cyy, cxy, det) = (cxx as f64, cyy as f64, cxy as f64, det as f64);
        Some(Self {
            x: (cxv * cyy - cyv * cxy) / det,
            y: (cyv * cxx - cxv * cxy) / det,
        })
    }
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
    pub(crate) const fn get(self, component: TileComponent) -> f32 {
        match component {
            TileComponent::Sky => self.sky,
            TileComponent::Sigma => self.sigma,
        }
    }

    pub(crate) const fn get_mut(&mut self, component: TileComponent) -> &mut f32 {
        match component {
            TileComponent::Sky => &mut self.sky,
            TileComponent::Sigma => &mut self.sigma,
        }
    }
}

impl TileStats {
    /// The statistic `component` names.
    pub(crate) const fn get(self, component: TileComponent) -> f32 {
        match component {
            TileComponent::Sky => self.sky,
            TileComponent::Sigma => self.sigma,
        }
    }

    /// Compute the sky, about the tile's own plane, and the raw clipped σ of the pixels of `tile`.
    ///
    /// When a mask is provided, only unmasked pixels are used. If all pixels
    /// are masked, falls back to sampling all pixels (including masked) as a
    /// last resort. A noisy estimate from few background pixels is far better
    /// than a biased estimate contaminated by star flux.
    ///
    /// The sky reads the samples less a plane fitted to the clip survivors — the departure from
    /// SExtractor, photutils and `NoiseChisel`, which take it from the raw pixels and leave the tile
    /// size to keep the sky's change across a tile small. On a sloped sky that change widens the
    /// raw clip past the wings of a bright star, and lets the Pearson mode read the star's skew
    /// against a σ the slope inflated, which pulls the sky down. About the plane the clip sits at
    /// the noise's width. The plane is zero at the tile's centre, so a planar sky's level stays.
    ///
    /// σ keeps the field's definition, the raw spread. The detector thresholds against it, and
    /// where the sky changes fast across a tile the mesh follows it worst: a σ about the plane
    /// drops the threshold under that model error, and on a vignetted or nebulous field the sky's
    /// residual then joins the stars into a few large regions that hide most of them.
    pub(crate) fn compute(
        pixels: &Buffer2<f32>,
        mask: Option<&BitBuffer2>,
        tile: URect,
        sigma_clip_iterations: usize,
        scratch: &mut TileScratch,
    ) -> Self {
        let TileScratch {
            values,
            offsets,
            detrended,
            deviations,
        } = scratch;
        values.clear();
        offsets.clear();

        match mask {
            Some(m) => {
                collect_unmasked_pixels(pixels, m, tile, values, offsets);
                if values.is_empty() {
                    // All pixels masked — no choice but to use all pixels
                    collect_tile_pixels(pixels, tile, values, offsets);
                }
            }
            None => collect_tile_pixels(pixels, tile, values, offsets),
        }

        if values.is_empty() {
            return Self::default();
        }

        let clipped = |slope: TileSlope, detrended: &mut Vec<f32>, deviations: &mut Vec<f32>| {
            detrended.clear();
            detrended.extend(
                values
                    .iter()
                    .zip(offsets.iter())
                    .map(|(&value, &offset)| (f64::from(value) - slope.at(offset)) as f32),
            );
            ClippedStats::sigma_clipped(detrended, deviations, CLIP_KAPPA, sigma_clip_iterations)
        };
        let mut slope = TileSlope::default();
        // A zero slope leaves every sample as it is: these are the raw statistics.
        let raw = clipped(slope, detrended, deviations);
        let mut stats = raw;
        for _ in 0..PLANE_FITS {
            let Some(fit) = slope.refit(values, offsets, stats.median, CLIP_KAPPA * stats.sigma)
            else {
                break;
            };
            slope = fit;
            stats = clipped(slope, detrended, deviations);
        }

        Self {
            sky: sextractor_sky(&stats),
            sigma: raw.sigma,
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

/// The `k`-th of `count` ordinals picked from `0..candidates`: evenly spread, and point-symmetric,
/// `k` and `count − 1 − k` summing to `candidates − 1`. Over a whole tile in raster order that
/// reflection is the one through the tile's centre, so the samples centre on it exactly and a
/// plane's statistics read the plane there. Picking from the first ordinal on instead puts the
/// samples' centre half a spacing early, and the sky a half-spacing step of its gradient off.
///
/// `count` is at most `candidates`, and even whenever `candidates` is: no self-symmetric middle
/// ordinal exists then. `count == candidates` picks every one.
const fn sample_ordinal(k: usize, count: usize, candidates: usize) -> usize {
    if 2 * k < count {
        ((2 * k + 1) * candidates - count) / (2 * count)
    } else {
        candidates - 1 - sample_ordinal(count - 1 - k, count, candidates)
    }
}

/// How many of `candidates` pixels a tile samples: all of them up to [`MAX_TILE_SAMPLES`], which is
/// even, as [`sample_ordinal`] needs.
const fn sample_count(candidates: usize) -> usize {
    if candidates < MAX_TILE_SAMPLES {
        candidates
    } else {
        MAX_TILE_SAMPLES
    }
}

/// The offset of pixel `(x, y)` from the centre of `tile`, doubled: an integer on a tile of any
/// parity, so the plane fit's position moments are exact.
fn centred(tile: URect, x: usize, y: usize) -> [i32; 2] {
    let axis = |at: usize, min: usize, side: usize| {
        let doubled = i32::try_from(2 * (at - min)).expect("a tile side is below 2^30");
        doubled - i32::try_from(side - 1).expect("a tile side is below 2^30")
    };
    [
        axis(x, tile.min.x, tile.width()),
        axis(y, tile.min.y, tile.height()),
    ]
}

/// Every pixel of `tile`, or [`MAX_TILE_SAMPLES`] of them picked by [`sample_ordinal`], each with
/// its [`centred`] offset.
fn collect_tile_pixels(
    pixels: &Buffer2<f32>,
    tile: URect,
    values: &mut Vec<f32>,
    offsets: &mut Vec<[i32; 2]>,
) {
    let width = pixels.width();
    let tile_width = tile.width();
    let candidates = tile.area();
    let count = sample_count(candidates);
    values.reserve_exact(count);
    offsets.reserve_exact(count);
    for k in 0..count {
        let ordinal = sample_ordinal(k, count, candidates);
        let (x, y) = (
            tile.min.x + ordinal % tile_width,
            tile.min.y + ordinal / tile_width,
        );
        values.push(pixels[y * width + x]);
        offsets.push(centred(tile, x, y));
    }
}

#[inline]
fn collect_unmasked_pixels(
    pixels: &Buffer2<f32>,
    mask: &BitBuffer2,
    tile: URect,
    values: &mut Vec<f32>,
    offsets: &mut Vec<[i32; 2]>,
) {
    let unmasked_count = count_unmasked_pixels(mask, tile);
    let sample_count = sample_count(unmasked_count);
    if sample_count == 0 {
        return;
    }

    let width = pixels.width();
    let mask_words = &mask.words;
    let words_per_row = mask.words_per_row();
    let mut next_ordinal = sample_ordinal(0, sample_count, unmasked_count);
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
                    let column = x + bits.trailing_zeros() as usize;
                    values.push(pixels[row_start + column]);
                    offsets.push(centred(tile, column, y));
                    selected_count += 1;
                    if selected_count == sample_count {
                        return;
                    }
                    next_ordinal = sample_ordinal(selected_count, sample_count, unmasked_count);
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
const fn unmasked_bits(mask_word: u64, bit_offset: usize, bits_to_process: usize) -> u64 {
    let relevant_bits = if bits_to_process == 64 {
        !0
    } else {
        ((1u64 << bits_to_process) - 1) << bit_offset
    };
    (!mask_word & relevant_bits) >> bit_offset
}

#[cfg(test)]
mod tests;
