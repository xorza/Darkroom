//! Local contrast enhancement via Contrast-Limited Adaptive Histogram Equalization (CLAHE).
//!
//! A **display-domain** (post-stretch, `[0,1]`) operation: tile the image, equalize each tile's
//! histogram with a clip limit (so flat regions aren't over-amplified), and bilinearly blend the
//! per-tile mappings. Brings out medium-scale structure (dust lanes, nebula filaments). Runs on the
//! combined intensity and scales channels by `f(I)/I` so hue is preserved.

use common::{Buffer2, Introspect, par};
use rayon::prelude::*;

use crate::config_field::ConfigField;
use crate::error::InvalidConfigField;
use crate::image::intensity::Intensity;
use crate::image::linear_image::LinearImage;
use crate::image_ops::error::OpError;

/// Histogram resolution for the per-tile mappings.
const BIN_COUNT: usize = 256;

/// Local contrast enhancement of a *stretched* (display-domain) image in place via CLAHE.
///
/// Computed on the combined intensity; color channels are rescaled hue-preservingly. Mono gets
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
    /// Enhance the local contrast of `image` in place via CLAHE.
    ///
    /// # Errors
    /// [`OpError::Config`] on out-of-range parameters.
    pub fn apply(&self, image: &mut LinearImage) -> Result<(), OpError> {
        self.validate()?;
        if self.strength == 0.0 {
            return Ok(());
        }
        image.remap_intensity(|intensity| self.map(intensity));
        Ok(())
    }

    /// The CLAHE mapping on the combined intensity plane; [`Self::apply`] computes the intensity,
    /// runs this, then remaps the image's channels to it.
    #[expect(clippy::cast_sign_loss, reason = "a square root is non-negative")]
    fn map(&self, intensity: Intensity<'_>) -> Buffer2<f32> {
        let size = intensity.size();
        // Keep each tile well-populated (≳ 4·N_BINS pixels) so the clipped-histogram CDF is
        // meaningful; on a small image this caps the requested tile count (a no-op on a real
        // megapixel frame).
        let max_tiles =
            ((size.pixel_count() as f64 / (4 * BIN_COUNT) as f64).sqrt() as usize).max(1);
        let tiles = self.tiles.min(max_tiles);
        let grid = TileGrid {
            cols: TileAxis::new(size.width, tiles),
            rows: TileAxis::new(size.height, tiles),
        };
        let luts = grid.luts(intensity, self.clip_limit);
        grid.apply(intensity, &luts, self.strength)
    }

    fn validate(&self) -> Result<(), InvalidConfigField> {
        ConfigField::count("local contrast tiles", self.tiles)
            .must_be("at least 1", self.tiles >= 1)?;
        ConfigField::new("local contrast clip_limit", self.clip_limit)
            .finite_and("finite and at least 1", |value| value >= 1.0)?;
        ConfigField::new("local contrast strength", self.strength)
            .finite_and("finite and in [0, 1]", |value| (0.0..=1.0).contains(&value))
    }
}

/// A tile's mapping: the clipped histogram's cumulative fraction at each bin edge, `BIN_COUNT + 1`
/// nodes from 0 to 1. Between edges the mapping is linear, the CDF of the histogram's own
/// piecewise-uniform density — so a value maps continuously, not to its bin's one level.
type TileLut = [f32; BIN_COUNT + 1];

/// Clip every bin at `clip` and hand the excess back evenly: an equal share to every bin, and the
/// remainder one apiece to bins spread across the range at a stride of `BIN_COUNT / remainder`, as
/// OpenCV's CLAHE does — given to the lowest bins instead, it would lift the dark end alone.
fn clip_histogram(hist: &mut [u32; BIN_COUNT], clip: u32) {
    let mut excess = 0u32;
    for c in hist.iter_mut() {
        if *c > clip {
            excess += *c - clip;
            *c = clip;
        }
    }
    let share = excess / BIN_COUNT as u32;
    let mut remainder = (excess % BIN_COUNT as u32) as usize;
    for c in hist.iter_mut() {
        *c += share;
    }
    if let Some(stride) = BIN_COUNT.checked_div(remainder) {
        let stride = stride.max(1);
        let mut bin = 0;
        while bin < BIN_COUNT && remainder > 0 {
            hist[bin] += 1;
            bin += stride;
            remainder -= 1;
        }
    }
}

/// How one axis of `extent` pixels splits into tiles: tile `t` covers `[t·extent/n,
/// (t+1)·extent/n)` — never empty, as at most `extent` tiles are made — and sits at the mean index
/// of its pixels.
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

    const fn tiles(&self) -> usize {
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

/// The tiles over a plane: how its columns and its rows split.
#[derive(Debug)]
struct TileGrid {
    cols: TileAxis,
    rows: TileAxis,
}

impl TileGrid {
    /// One mapping per tile, row-major.
    #[expect(
        clippy::cast_sign_loss,
        reason = "the clip limit is at least 1 by construction"
    )]
    fn luts(&self, intensity: Intensity<'_>, clip_limit: f32) -> Vec<TileLut> {
        let width = intensity.size().width;
        let Self { cols, rows } = self;
        let mut luts = vec![[0.0f32; BIN_COUNT + 1]; cols.tiles() * rows.tiles()];
        par::iter_mut(&mut luts).enumerate().for_each(|(idx, lut)| {
            let (tx, ty) = (idx % cols.tiles(), idx / cols.tiles());
            let (x0, x1) = (cols.bounds[tx], cols.bounds[tx + 1]);
            let mut hist = [0u32; BIN_COUNT];
            for y in rows.bounds[ty]..rows.bounds[ty + 1] {
                for idx in y * width + x0..y * width + x1 {
                    if !intensity.is_fill(idx) {
                        hist[bin_of(intensity.at(idx))] += 1;
                    }
                }
            }
            let count: u32 = hist.iter().sum();
            // A tile of fills alone has no distribution to equalize, and keeps every level.
            if count == 0 {
                for (b, level) in lut.iter_mut().enumerate() {
                    *level = b as f32 / BIN_COUNT as f32;
                }
                return;
            }
            let clip = (clip_limit * count as f32 / BIN_COUNT as f32).max(1.0) as u32;
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

    /// Map each pixel's intensity through the bilinearly-interpolated four-tile mapping, blended
    /// with the original by `strength`.
    fn apply(&self, intensity: Intensity<'_>, luts: &[TileLut], strength: f32) -> Buffer2<f32> {
        let Self { cols, rows } = self;
        let size = intensity.size();
        let col_blends: Vec<Blend> = (0..size.width).map(|x| cols.blend(x)).collect();
        let mut out = Buffer2::new_default(size);
        par::chunks_mut(out.pixels_mut(), size.width)
            .enumerate()
            .for_each(|(y, orow)| {
                let row = rows.blend(y);
                let (top, bottom) = (row.lower * cols.tiles(), row.upper * cols.tiles());
                for (x, (o, col)) in orow.iter_mut().zip(&col_blends).enumerate() {
                    let v = intensity.at(y * size.width + x);
                    let at = |tile: usize| map_through(&luts[tile], v);
                    let upper_row = at(top + col.lower)
                        + col.weight * (at(top + col.upper) - at(top + col.lower));
                    let lower_row = at(bottom + col.lower)
                        + col.weight * (at(bottom + col.upper) - at(bottom + col.lower));
                    let mapped = upper_row + row.weight * (lower_row - upper_row);
                    *o = v + strength * (mapped - v);
                }
            });
        out
    }
}

/// `v` through `lut`, linear between its bin edges.
#[inline]
#[expect(
    clippy::cast_sign_loss,
    reason = "the value is clamped into [0, 1] first"
)]
fn map_through(lut: &TileLut, v: f32) -> f32 {
    let pos = v.clamp(0.0, 1.0) * BIN_COUNT as f32;
    let bin = (pos as usize).min(BIN_COUNT - 1);
    let fraction = pos - bin as f32;
    lut[bin] + fraction * (lut[bin + 1] - lut[bin])
}

/// The histogram bin of `v`: `[b/N, (b+1)/N)`, 1 in the last bin.
#[inline]
#[expect(
    clippy::cast_sign_loss,
    reason = "the value is clamped into [0, 1] first"
)]
fn bin_of(v: f32) -> usize {
    ((v.clamp(0.0, 1.0) * BIN_COUNT as f32) as usize).min(BIN_COUNT - 1)
}

#[cfg(test)]
mod tests;
