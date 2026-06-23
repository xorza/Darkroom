//! Local contrast enhancement via Contrast-Limited Adaptive Histogram Equalization (CLAHE). See
//! `local_contrast/README.md` for the algorithm.
//!
//! A **display-domain** (post-stretch, `[0,1]`) operation: tile the image, equalize each tile's
//! histogram with a clip limit (so flat regions aren't over-amplified), and bilinearly blend the
//! per-tile mappings. Brings out medium-scale structure (dust lanes, nebula filaments). Runs on the
//! combined intensity and scales channels by `f(I)/I` so hue is preserved.

use imaginarium::Buffer2;
use rayon::prelude::*;

use crate::image_ops::remap_intensity;
use imaginarium::Image;

#[cfg(test)]
mod tests;

/// Histogram resolution for the per-tile mappings.
const N_BINS: usize = 256;

/// Parameters for [`enhance_local_contrast`].
#[derive(Debug, Clone, Copy)]
pub struct LocalContrastConfig {
    /// Tile grid count per axis. Fewer/larger tiles = broader structure; ~8 is typical, lower for
    /// wide-field.
    pub tiles: usize,
    /// Histogram clip limit (`≥ 1`): the per-level amplification cap. `1` ≈ no enhancement; 2–4
    /// typical. The "contrast-limited" knob that stops flat/noisy regions from blowing up.
    pub clip_limit: f32,
    /// Blend with the original in `[0, 1]`: `1` = full CLAHE, `0` = identity.
    pub strength: f32,
}

impl Default for LocalContrastConfig {
    fn default() -> Self {
        Self {
            tiles: 8,
            clip_limit: 2.0,
            strength: 0.8,
        }
    }
}

impl LocalContrastConfig {
    /// Panic on out-of-range parameters (called by [`enhance_local_contrast`]).
    pub fn validate(&self) {
        assert!(self.tiles >= 1, "tiles must be ≥ 1, got {}", self.tiles);
        assert!(
            self.clip_limit >= 1.0 && self.clip_limit.is_finite(),
            "clip_limit must be ≥ 1, got {}",
            self.clip_limit
        );
        assert!(
            (0.0..=1.0).contains(&self.strength),
            "strength must be in [0, 1], got {}",
            self.strength
        );
    }
}

/// Enhance local contrast of a *stretched* (display-domain) image in place via CLAHE.
///
/// Computed on the combined intensity; color channels are rescaled hue-preservingly. Grayscale gets
/// the mapping directly.
pub fn enhance_local_contrast(image: &mut Image, config: LocalContrastConfig) {
    config.validate();
    remap_intensity(image, |intensity| clahe_map(intensity, config));
}

/// The CLAHE mapping on the combined intensity plane; [`enhance_local_contrast`]
/// computes the intensity, runs this, then remaps the image's channels to it.
fn clahe_map(intensity: &Buffer2<f32>, config: LocalContrastConfig) -> Buffer2<f32> {
    // Keep each tile well-populated (≳ 4·N_BINS pixels) so the clipped-histogram CDF is meaningful;
    // on a small image this caps the requested tile count (a no-op on a real megapixel frame).
    let max_tiles = (((intensity.width() * intensity.height()) as f64 / (4 * N_BINS) as f64).sqrt()
        as usize)
        .max(1);
    let tiles = config.tiles.min(max_tiles);
    let luts = build_tile_luts(intensity, tiles, config.clip_limit);
    apply_luts(intensity, &luts, tiles, config.strength)
}

/// One mapping LUT per tile (`tiles*tiles`, row-major), each `bin → [0,1]` from the clipped CDF.
fn build_tile_luts(intensity: &Buffer2<f32>, tiles: usize, clip_limit: f32) -> Vec<[f32; N_BINS]> {
    let (w, h) = (intensity.width(), intensity.height());
    let (tw, th) = (w.div_ceil(tiles), h.div_ceil(tiles));
    let mut luts = vec![[0.0f32; N_BINS]; tiles * tiles];
    luts.par_iter_mut().enumerate().for_each(|(idx, lut)| {
        let (tx, ty) = (idx % tiles, idx / tiles);
        let (x0, x1) = (tx * tw, ((tx + 1) * tw).min(w));
        let (y0, y1) = (ty * th, ((ty + 1) * th).min(h));

        let mut hist = [0u32; N_BINS];
        let mut count = 0u32;
        for y in y0..y1 {
            for &v in &intensity.row(y)[x0..x1] {
                hist[bin_of(v)] += 1;
                count += 1;
            }
        }
        if count == 0 {
            // Empty tile (div_ceil overshoot at an edge): identity mapping.
            for (b, e) in lut.iter_mut().enumerate() {
                *e = b as f32 / (N_BINS as f32 - 1.0);
            }
            return;
        }

        // Clip each bin at the limit and redistribute the excess equally (contrast limiting).
        let clip = (clip_limit * count as f32 / N_BINS as f32).max(1.0) as u32;
        let mut excess = 0u32;
        for c in hist.iter_mut() {
            if *c > clip {
                excess += *c - clip;
                *c = clip;
            }
        }
        let add = excess / N_BINS as u32;
        let rem = excess % N_BINS as u32;
        for (b, c) in hist.iter_mut().enumerate() {
            *c += add + u32::from((b as u32) < rem);
        }

        // Normalized CDF → mapping.
        let total: u32 = hist.iter().sum();
        let mut cum = 0u32;
        for (b, &c) in hist.iter().enumerate() {
            cum += c;
            lut[b] = cum as f32 / total as f32;
        }
    });
    luts
}

/// Map each pixel's intensity through the bilinearly-interpolated four-tile mapping, blended with
/// the original by `strength`.
fn apply_luts(
    intensity: &Buffer2<f32>,
    luts: &[[f32; N_BINS]],
    tiles: usize,
    strength: f32,
) -> Buffer2<f32> {
    let (w, h) = (intensity.width(), intensity.height());
    let (tw, th) = (w.div_ceil(tiles) as f32, h.div_ceil(tiles) as f32);
    let last = tiles as f32 - 1.0;
    let mut out = Buffer2::new_default(w, h);
    out.pixels_mut()
        .par_chunks_mut(w)
        .enumerate()
        .for_each(|(y, orow)| {
            // Fractional tile-centre position of this row (tile ty centred at (ty+0.5)·th).
            let gy = ((y as f32 + 0.5) / th - 0.5).clamp(0.0, last);
            let ty0 = gy.floor() as usize;
            let ty1 = (ty0 + 1).min(tiles - 1);
            let fy = gy - ty0 as f32;
            let irow = intensity.row(y);
            for x in 0..w {
                let v = irow[x];
                let b = bin_of(v);
                let gx = ((x as f32 + 0.5) / tw - 0.5).clamp(0.0, last);
                let tx0 = gx.floor() as usize;
                let tx1 = (tx0 + 1).min(tiles - 1);
                let fx = gx - tx0 as f32;
                let tl = luts[ty0 * tiles + tx0][b];
                let tr = luts[ty0 * tiles + tx1][b];
                let bl = luts[ty1 * tiles + tx0][b];
                let br = luts[ty1 * tiles + tx1][b];
                let top = tl + fx * (tr - tl);
                let bot = bl + fx * (br - bl);
                let mapped = top + fy * (bot - top);
                orow[x] = v + strength * (mapped - v);
            }
        });
    out
}

#[inline]
fn bin_of(v: f32) -> usize {
    (v.clamp(0.0, 1.0) * (N_BINS as f32 - 1.0)).round() as usize
}
