//! Local contrast enhancement via Contrast-Limited Adaptive Histogram Equalization (CLAHE).
//!
//! A **display-domain** (post-stretch, `[0,1]`) operation: tile the image, equalize each tile's
//! histogram with a clip limit (so flat regions aren't over-amplified), and bilinearly blend the
//! per-tile mappings. Brings out medium-scale structure (dust lanes, nebula filaments). Runs on the
//! combined intensity and scales channels by `f(I)/I` so hue is preserved.

use crate::math::size2us::Size2us;
use common::Introspect;
use imaginarium::Buffer2;
use rayon::prelude::*;

use crate::error::InvalidConfigField;
use crate::image_ops::error::OpError;
use crate::io::image::linear::LinearImage;

/// Histogram resolution for the per-tile mappings.
const N_BINS: usize = 256;

/// Local contrast enhancement of a *stretched* (display-domain) image in place via CLAHE.
///
/// Computed on the combined intensity; color channels are rescaled hue-preservingly. Grayscale gets
/// the mapping directly.
#[derive(Debug, Clone, Copy, Introspect)]
#[config(type_id = "eb0062ca-cef9-4fef-a52b-cf3e8e0fce3c")]
pub struct LocalContrast {
    /// Tile grid count per axis. Fewer/larger tiles = broader structure; ~8 is typical, lower for
    /// wide-field.
    pub tiles: usize,
    /// Histogram clip limit (`≥ 1`): the per-level amplification cap. `1` ≈ no enhancement; 2–4
    /// typical. The "contrast-limited" knob that stops flat/noisy regions from blowing up.
    pub clip_limit: f32,
    /// Blend with the original in `[0, 1]`: `1` = full CLAHE, `0` = identity.
    pub strength: f32,
}

impl Default for LocalContrast {
    fn default() -> Self {
        Self {
            tiles: 8,
            clip_limit: 2.0,
            strength: 0.8,
        }
    }
}

impl LocalContrast {
    /// Set the tile grid count per axis.
    #[must_use]
    pub fn tiles(mut self, tiles: usize) -> Self {
        self.tiles = tiles;
        self
    }

    /// Set the histogram clip limit (`≥ 1`).
    #[must_use]
    pub fn clip_limit(mut self, clip_limit: f32) -> Self {
        self.clip_limit = clip_limit;
        self
    }

    /// Set the CLAHE/original blend in `[0, 1]`.
    #[must_use]
    pub fn strength(mut self, strength: f32) -> Self {
        self.strength = strength;
        self
    }

    /// Enhance the local contrast of `image` in place via CLAHE.
    ///
    /// # Errors
    /// [`OpError::InvalidConfig`] on out-of-range parameters.
    pub fn apply(&self, image: &mut LinearImage) -> Result<(), OpError> {
        self.validate()?;
        if self.strength == 0.0 {
            return Ok(());
        }
        image.remap_intensity(|intensity| clahe_map(intensity, self));
        Ok(())
    }

    fn validate(&self) -> Result<(), InvalidConfigField> {
        InvalidConfigField::check(
            self.tiles >= 1,
            "local contrast tiles",
            "at least 1",
            self.tiles as f64,
        )?;
        InvalidConfigField::finite(
            "local contrast clip_limit",
            "finite and at least 1",
            self.clip_limit,
            |value| value >= 1.0,
        )?;
        InvalidConfigField::finite(
            "local contrast strength",
            "finite and in [0, 1]",
            self.strength,
            |value| (0.0..=1.0).contains(&value),
        )
    }
}

/// The CLAHE mapping on the combined intensity plane; [`LocalContrast::apply`] computes the
/// intensity, runs this, then remaps the image's channels to it.
fn clahe_map(intensity: &Buffer2<f32>, config: &LocalContrast) -> Buffer2<f32> {
    // Keep each tile well-populated (≳ 4·N_BINS pixels) so the clipped-histogram CDF is meaningful;
    // on a small image this caps the requested tile count (a no-op on a real megapixel frame).
    let max_tiles = (((intensity.width() * intensity.height()) as f64 / (4 * N_BINS) as f64).sqrt()
        as usize)
        .max(1);
    let tiles = config.tiles.min(max_tiles);
    let columns = TileAxis::new(intensity.width(), tiles);
    let rows = TileAxis::new(intensity.height(), tiles);
    let luts = build_tile_luts(intensity, &columns, &rows, config.clip_limit);
    apply_luts(intensity, &luts, &columns, &rows, config.strength)
}

/// A tile's mapping: the clipped histogram's cumulative fraction at each bin edge, `N_BINS + 1`
/// nodes from 0 to 1. Between edges the mapping is linear, the CDF of the histogram's own
/// piecewise-uniform density — so a value maps continuously, not to its bin's one level.
type TileLut = [f32; N_BINS + 1];

/// One mapping per tile, row-major.
fn build_tile_luts(
    intensity: &Buffer2<f32>,
    columns: &TileAxis,
    rows: &TileAxis,
    clip_limit: f32,
) -> Vec<TileLut> {
    let mut luts = vec![[0.0f32; N_BINS + 1]; columns.tiles() * rows.tiles()];
    luts.par_iter_mut().enumerate().for_each(|(idx, lut)| {
        let (tx, ty) = (idx % columns.tiles(), idx / columns.tiles());
        let (x0, x1) = (columns.bounds[tx], columns.bounds[tx + 1]);
        let mut hist = [0u32; N_BINS];
        for y in rows.bounds[ty]..rows.bounds[ty + 1] {
            for &v in &intensity.row(y)[x0..x1] {
                hist[bin_of(v)] += 1;
            }
        }
        let count: u32 = hist.iter().sum();
        let clip = (clip_limit * count as f32 / N_BINS as f32).max(1.0) as u32;
        clip_histogram(&mut hist, clip);
        let total: u32 = hist.iter().sum();
        let mut cum = 0u32;
        lut[0] = 0.0;
        for (b, &c) in hist.iter().enumerate() {
            cum += c;
            lut[b + 1] = cum as f32 / total as f32;
        }
    });
    luts
}

/// Clip every bin at `clip` and hand the excess back evenly: an equal share to every bin, and the
/// remainder one apiece to bins spread across the range at a stride of `N_BINS / remainder`, as
/// OpenCV's CLAHE does — given to the lowest bins instead, it would lift the dark end alone.
fn clip_histogram(hist: &mut [u32; N_BINS], clip: u32) {
    let mut excess = 0u32;
    for c in hist.iter_mut() {
        if *c > clip {
            excess += *c - clip;
            *c = clip;
        }
    }
    let share = excess / N_BINS as u32;
    let mut remainder = (excess % N_BINS as u32) as usize;
    for c in hist.iter_mut() {
        *c += share;
    }
    if let Some(stride) = N_BINS.checked_div(remainder) {
        let stride = stride.max(1);
        let mut bin = 0;
        while bin < N_BINS && remainder > 0 {
            hist[bin] += 1;
            bin += stride;
            remainder -= 1;
        }
    }
}

/// How one axis of `extent` pixels splits into tiles: tile `t` covers `[t·extent/n, (t+1)·extent/n)`
/// — never empty, as at most `extent` tiles are made — and sits at the mean index of its pixels.
#[derive(Debug)]
struct TileAxis {
    /// The tiles' edges, `n + 1` of them.
    bounds: Vec<usize>,
    centres: Vec<f32>,
}

/// The two tiles a coordinate blends between, and the weight of the second.
#[derive(Debug, Clone, Copy)]
struct Blend {
    lower: usize,
    upper: usize,
    weight: f32,
}

impl TileAxis {
    fn new(extent: usize, tiles: usize) -> Self {
        let tiles = tiles.clamp(1, extent);
        let bounds: Vec<usize> = (0..=tiles).map(|t| t * extent / tiles).collect();
        let centres = bounds
            .windows(2)
            .map(|edge| (edge[0] + edge[1] - 1) as f32 * 0.5)
            .collect();
        Self { bounds, centres }
    }

    fn tiles(&self) -> usize {
        self.centres.len()
    }

    /// The tiles whose centres bracket `x`, the nearer edge tile alone past the outer centres.
    fn blend(&self, x: usize) -> Blend {
        let x = x as f32;
        let upper = self.centres.partition_point(|&centre| centre <= x);
        if upper == 0 || upper == self.tiles() {
            let edge = upper.min(self.tiles() - 1);
            return Blend {
                lower: edge,
                upper: edge,
                weight: 0.0,
            };
        }
        let (c0, c1) = (self.centres[upper - 1], self.centres[upper]);
        Blend {
            lower: upper - 1,
            upper,
            weight: (x - c0) / (c1 - c0),
        }
    }
}

/// Map each pixel's intensity through the bilinearly-interpolated four-tile mapping, blended with
/// the original by `strength`.
fn apply_luts(
    intensity: &Buffer2<f32>,
    luts: &[TileLut],
    columns: &TileAxis,
    rows: &TileAxis,
    strength: f32,
) -> Buffer2<f32> {
    let size = Size2us::new(intensity.width(), intensity.height());
    let column_blends: Vec<Blend> = (0..size.width).map(|x| columns.blend(x)).collect();
    let mut out = Buffer2::new_default(size.width, size.height);
    out.pixels_mut()
        .par_chunks_mut(size.width)
        .enumerate()
        .for_each(|(y, orow)| {
            let row = rows.blend(y);
            let (top, bottom) = (row.lower * columns.tiles(), row.upper * columns.tiles());
            for ((o, &v), column) in orow.iter_mut().zip(intensity.row(y)).zip(&column_blends) {
                let at = |tile: usize| map_through(&luts[tile], v);
                let upper_row = at(top + column.lower)
                    + column.weight * (at(top + column.upper) - at(top + column.lower));
                let lower_row = at(bottom + column.lower)
                    + column.weight * (at(bottom + column.upper) - at(bottom + column.lower));
                let mapped = upper_row + row.weight * (lower_row - upper_row);
                *o = v + strength * (mapped - v);
            }
        });
    out
}

/// `v` through `lut`, linear between its bin edges.
#[inline]
fn map_through(lut: &TileLut, v: f32) -> f32 {
    let position = v.clamp(0.0, 1.0) * N_BINS as f32;
    let bin = (position as usize).min(N_BINS - 1);
    let fraction = position - bin as f32;
    lut[bin] + fraction * (lut[bin + 1] - lut[bin])
}

/// The histogram bin of `v`: `[b/N, (b+1)/N)`, 1 in the last bin.
#[inline]
fn bin_of(v: f32) -> usize {
    ((v.clamp(0.0, 1.0) * N_BINS as f32) as usize).min(N_BINS - 1)
}

#[cfg(test)]
mod tests;
