//! Per-tile SExtractor sky estimation: pixel collection (masked/sampled), sigma-clipped robust
//! statistics, read about the tile's own plane for the sky, and the crowding-aware Pearson-mode
//! sky estimator for a single tile box.

use common::Buffer2;

use crate::bit_buffer2::BitBuffer2;
use crate::image::cfa::mosaic::Mosaic;
use crate::math::plane_moments::PlaneMoments;
use crate::math::statistics::clipped_stats::{ClippedStats, SigmaClip};
use crate::scratch_pool::Shapeless;
use common::Rect2us;
use glam::{DVec2, IVec2, USizeVec2};

/// Maximum samples per tile for statistics computation.
pub(super) const MAX_TILE_SAMPLES: usize = 1024;
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

/// What every tile of a mesh is measured from: the plane, the mask that leaves pixels out, the
/// color of a mosaic the tiles read alone, and the σ-clip passes of each tile.
#[derive(Debug, Clone, Copy)]
pub(super) struct TileInput<'a> {
    pub(super) pixels: &'a Buffer2<f32>,
    pub(super) mask: Option<&'a BitBuffer2>,
    pub(super) color: Option<ColorSites>,
    pub(super) sigma_clip_max_iterations: usize,
}

/// One color of a mosaic: the photosites of `mosaic` of color `color`.
#[derive(Debug, Clone, Copy)]
pub(crate) struct ColorSites {
    pub(crate) mosaic: Mosaic,
    pub(crate) color: usize,
}

impl ColorSites {
    const fn takes(self, pos: USizeVec2) -> bool {
        self.mosaic.slot_at(pos) == self.color
    }
}

/// A tile's working buffers, kept across tiles by the mesh's scratch pool.
#[derive(Debug, Default)]
pub(super) struct TileScratch {
    pub(super) samples: TileSamples,
    /// The samples less the tile's plane, reordered by the clip.
    detrended: Vec<f32>,
    pub(super) deviations: Vec<f32>,
}

impl Shapeless for TileScratch {}

/// A tile's samples, each with its doubled offset from the tile's centre (see [`centred`]).
#[derive(Debug, Default)]
pub(super) struct TileSamples {
    pub(super) values: Vec<f32>,
    offsets: Vec<IVec2>,
}

/// Tile statistics computed during background estimation.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct TileStats {
    /// Sky level at the tile's centre: SExtractor's crowding-aware estimator (Pearson mode, median
    /// fallback when strongly skewed) over the survivors of a clip about the tile's own plane.
    /// Computed by [`TileStats::compute`].
    pub(crate) sky: f32,
    /// The background RMS: the spread of the clip survivors of the raw samples, which counts the
    /// sky's own change across the tile as well as the noise. SExtractor takes the survivors'
    /// standard deviation; this is their MAD's σ, which a star's wing the clip left does not move.
    pub(crate) sigma: f32,
    /// The spread of the clip survivors about the tile's plane: the noise alone, which a gradient
    /// across the tile does not inflate.
    pub(crate) noise: f32,
}

/// The slope of a plane over one tile, per unit of a doubled, centred offset (see [`centred`]).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct TileSlope(DVec2);

impl TileSlope {
    /// The plane's rise from the tile's centre to the sample at `offset`.
    fn at(self, offset: IVec2) -> f64 {
        self.0.dot(offset.as_dvec2())
    }

    /// The least-squares slope of the `samples` whose value off this plane lies within `band` of
    /// `center`. `None` when those samples do not span a plane: fewer than three, or all on one
    /// line.
    fn refit(self, samples: &TileSamples, center: f32, band: f32) -> Option<Self> {
        let mut moments = PlaneMoments::default();
        for (&value, &offset) in samples.values.iter().zip(&samples.offsets) {
            let value = f64::from(value);
            if (value - self.at(offset) - f64::from(center)).abs() <= f64::from(band) {
                moments.add(offset, value);
            }
        }
        Some(Self(moments.plane()?.slope))
    }
}

/// Which of a tile's statistics a spline interpolates.
///
/// The sky and sigma planes are interpolated by identical code over identical grids; naming the
/// plane rather than passing a `fn(&TileStats) -> f32` is what keeps that one loop instead of two.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TileComponent {
    Sky,
    Sigma,
}

impl TileStats {
    /// The statistic `component` names.
    pub(crate) const fn get(self, component: TileComponent) -> f32 {
        match component {
            TileComponent::Sky => self.sky,
            TileComponent::Sigma => self.sigma,
        }
    }

    /// Compute the sky, about the tile's own plane, and the raw clipped σ of the pixels of `tile`,
    /// or `None` for a bad tile: one whose mask leaves less than half of it.
    ///
    /// When a mask is provided, only unmasked pixels are used. Below half the tile, the unmasked
    /// pixels are the margins of whatever the mask covers, and they no longer span the tile the
    /// plane fit and the sky's position assume; the grid takes such a tile from its good neighbours
    /// instead, as photutils excludes boxes past `exclude_percentile` and SExtractor flags bad
    /// meshes.
    ///
    /// The sky reads the samples less a plane fitted to the clip survivors — the departure from
    /// SExtractor, photutils and `NoiseChisel`, which take it from the raw pixels and leave the
    /// tile size to keep the sky's change across a tile small. On a sloped sky that change widens
    /// the raw clip past the wings of a bright star, and lets the Pearson mode read the star's skew
    /// against a σ the slope inflated, which pulls the sky down. About the plane the clip sits at
    /// the noise's width. The plane is zero at the tile's centre, so a planar sky's level stays.
    ///
    /// σ keeps the field's definition, the raw spread. The detector thresholds against it, and
    /// where the sky changes fast across a tile the mesh follows it worst: a σ about the plane
    /// drops the threshold under that model error, and on a vignetted or nebulous field the sky's
    /// residual then joins the stars into a few large regions that hide most of them.
    /// `input.color`, when given, limits the tile to the photosites of that color; the half is
    /// then of those photosites.
    pub(super) fn compute(
        input: &TileInput<'_>,
        tile: Rect2us,
        scratch: &mut TileScratch,
    ) -> Option<Self> {
        let TileScratch {
            samples,
            detrended,
            deviations,
        } = scratch;
        samples.values.clear();
        samples.offsets.clear();

        match (input.mask, input.color) {
            (mask, Some(color)) => {
                let selection = ColorTile { mask, color, tile };
                let counted = selection.count();
                if counted.unmasked == 0 || 2 * counted.unmasked < counted.eligible {
                    return None;
                }
                samples.collect_eligible(input.pixels, &selection, counted.unmasked);
            }
            (Some(m), None) => {
                let unmasked = count_unmasked_pixels(m, tile);
                if 2 * unmasked < tile.area() {
                    return None;
                }
                samples.collect_unmasked(input.pixels, m, tile, unmasked);
            }
            (None, None) => samples.collect_tile(input.pixels, tile),
        }
        debug_assert!(
            !samples.values.is_empty(),
            "a tile holds at least one pixel"
        );

        let samples = &*samples;
        let clipped = |slope: TileSlope, detrended: &mut Vec<f32>, deviations: &mut Vec<f32>| {
            detrended.clear();
            detrended.extend(
                samples
                    .values
                    .iter()
                    .zip(&samples.offsets)
                    .map(|(&value, &offset)| (f64::from(value) - slope.at(offset)) as f32),
            );
            ClippedStats::sigma_clipped(
                detrended,
                deviations,
                SigmaClip {
                    kappa: CLIP_KAPPA,
                    iterations: input.sigma_clip_max_iterations,
                },
            )
        };
        let mut slope = TileSlope::default();
        // A zero slope leaves every sample as it is: these are the raw statistics.
        let raw = clipped(slope, detrended, deviations);
        let mut stats = raw;
        for _ in 0..PLANE_FITS {
            let Some(fit) = slope.refit(samples, stats.median, CLIP_KAPPA * stats.sigma) else {
                break;
            };
            slope = fit;
            stats = clipped(slope, detrended, deviations);
        }

        Some(Self {
            sky: sextractor_sky(&stats),
            sigma: raw.sigma,
            noise: stats.sigma,
        })
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
fn centred(tile: Rect2us, x: usize, y: usize) -> IVec2 {
    let axis = |at: usize, min: usize, side: usize| {
        let doubled = i32::try_from(2 * (at - min)).expect("a tile side is below 2^30");
        doubled - i32::try_from(side - 1).expect("a tile side is below 2^30")
    };
    IVec2::new(
        axis(x, tile.min.x, tile.width()),
        axis(y, tile.min.y, tile.height()),
    )
}

impl TileSamples {
    /// Every pixel of `tile`, or [`MAX_TILE_SAMPLES`] of them picked by [`sample_ordinal`], each
    /// with its [`centred`] offset.
    fn collect_tile(&mut self, pixels: &Buffer2<f32>, tile: Rect2us) {
        let width = pixels.width();
        let tile_width = tile.width();
        let candidates = tile.area();
        let count = sample_count(candidates);
        self.values.reserve_exact(count);
        self.offsets.reserve_exact(count);
        for k in 0..count {
            let ordinal = sample_ordinal(k, count, candidates);
            let (x, y) = (
                tile.min.x + ordinal % tile_width,
                tile.min.y + ordinal / tile_width,
            );
            self.values.push(pixels[y * width + x]);
            self.offsets.push(centred(tile, x, y));
        }
    }

    /// The `unmasked_count` pixels of `tile` that `mask` leaves, or [`MAX_TILE_SAMPLES`] of them
    /// picked by [`sample_ordinal`], each with its [`centred`] offset.
    #[inline]
    fn collect_unmasked(
        &mut self,
        pixels: &Buffer2<f32>,
        mask: &BitBuffer2,
        tile: Rect2us,
        unmasked_count: usize,
    ) {
        debug_assert_eq!(unmasked_count, count_unmasked_pixels(mask, tile));
        let sample_count = sample_count(unmasked_count);
        if sample_count == 0 {
            return;
        }

        let width = pixels.width();
        let mut next_ordinal = sample_ordinal(0, sample_count, unmasked_count);
        let mut ordinal = 0;
        let mut selected_count = 0;

        for y in tile.min.y..tile.max.y {
            let row_start = y * width;
            let mask_row = mask.row_words(y);
            let mut x = tile.min.x;

            while x < tile.max.x {
                let word_idx = x / 64;
                let bit_offset = x % 64;
                let mask_word = mask_row[word_idx];
                let bits_to_process = (64 - bit_offset).min(tile.max.x - x);
                let mut bits = unmasked_bits(mask_word, bit_offset, bits_to_process);
                while bits != 0 {
                    if ordinal == next_ordinal {
                        let col = x + bits.trailing_zeros() as usize;
                        self.values.push(pixels[row_start + col]);
                        self.offsets.push(centred(tile, col, y));
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

    /// The pixels `selection` takes, `candidates` of them, or [`MAX_TILE_SAMPLES`] picked by
    /// [`sample_ordinal`], each with its [`centred`] offset.
    fn collect_eligible(
        &mut self,
        pixels: &Buffer2<f32>,
        selection: &ColorTile<'_>,
        candidates: usize,
    ) {
        let count = sample_count(candidates);
        self.values.reserve_exact(count);
        self.offsets.reserve_exact(count);
        let tile = selection.tile;
        let mut ordinal = 0;
        for y in tile.min.y..tile.max.y {
            for x in tile.min.x..tile.max.x {
                if !selection.takes(USizeVec2::new(x, y)) {
                    continue;
                }
                if self.values.len() < count
                    && ordinal == sample_ordinal(self.values.len(), count, candidates)
                {
                    self.values.push(pixels[(x, y)]);
                    self.offsets.push(centred(tile, x, y));
                }
                ordinal += 1;
            }
        }
        debug_assert_eq!(self.values.len(), count);
    }
}

/// One tile's photosites of one color, and the mask that leaves some of them out.
#[derive(Debug)]
struct ColorTile<'a> {
    mask: Option<&'a BitBuffer2>,
    color: ColorSites,
    tile: Rect2us,
}

/// A tile's photosites of the color, and how many of those the mask leaves.
#[derive(Debug, Clone, Copy)]
struct EligibleCount {
    eligible: usize,
    unmasked: usize,
}

impl ColorTile<'_> {
    fn count(&self) -> EligibleCount {
        let mut count = EligibleCount {
            eligible: 0,
            unmasked: 0,
        };
        for y in self.tile.min.y..self.tile.max.y {
            for x in self.tile.min.x..self.tile.max.x {
                let pos = USizeVec2::new(x, y);
                if self.color.takes(pos) {
                    count.eligible += 1;
                    count.unmasked += usize::from(!self.masked(pos));
                }
            }
        }
        count
    }

    fn takes(&self, pos: USizeVec2) -> bool {
        self.color.takes(pos) && !self.masked(pos)
    }

    fn masked(&self, pos: USizeVec2) -> bool {
        self.mask.is_some_and(|mask| mask.get_at(pos))
    }
}

#[inline]
fn count_unmasked_pixels(mask: &BitBuffer2, tile: Rect2us) -> usize {
    let mut count = 0;

    for y in tile.min.y..tile.max.y {
        let mask_row = mask.row_words(y);
        let mut x = tile.min.x;
        while x < tile.max.x {
            let word_idx = x / 64;
            let bit_offset = x % 64;
            let bits_to_process = (64 - bit_offset).min(tile.max.x - x);
            count += unmasked_bits(mask_row[word_idx], bit_offset, bits_to_process).count_ones()
                as usize;
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
