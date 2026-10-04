//! RCD (Ratio Corrected Demosaicing) algorithm.
//!
//! Based on: Luis Sanz Rodriguez, "Ratio Corrected Demosaicing" v2.3 (2017).
//! Reference: <https://github.com/LuisSR/RCD-Demosaicing>
//!
//! The algorithm uses directional discrimination and ratio-corrected
//! interpolation in a low-pass filter domain to reduce color artifacts,
//! particularly beneficial for astrophotography (star morphology). Near a signed
//! low-pass denominator cancellation, the ratio estimate blends continuously into
//! an additive midpoint estimate.

mod tile;

use std::ops::Range;

use common::CancelToken;
use rayon::prelude::*;

use crate::concurrency::unsafe_send_ptr::UnsafeSendPtr;
use crate::io::cancelled::Cancelled;
use crate::io::raw::demosaic::DemosaicMemory;
use crate::io::raw::demosaic::bayer::BayerImage;
use crate::io::raw::demosaic::bayer::rcd::tile::{OutputPlanes, Tile, TilePlace};
use crate::math::size2us::Size2us;
use crate::math::vec2us::Vec2us;

const EPS: f32 = 1e-5;
const EPSSQ: f32 = 1e-10;
// Limit the ratio's relative condition number to four before blending.
const MIN_SIGNED_DENOMINATOR_RATIO: f32 = 0.25;
/// Border size required by the algorithm (pixels on each side).
const BORDER: usize = 4;
/// The band bilinear interpolation fills. RCD's stages chain stencils — the direction maps and
/// low-pass filter reach 4 pixels, and the colour steps read values earlier steps computed up to 3
/// pixels further out — so a pixel nearer an edge than this reads values no stage computed. A
/// test pins the reach: from this distance in, a frame demosaics bit for bit as it does inside a
/// larger one. `RawTherapee`'s RCD interpolates a 9-pixel border for the same reason.
pub(crate) const INTERPOLATED_BORDER: usize = 10;

/// The side of a tile, where its seven planes stay in a core's cache, and even, so that every tile
/// starts on the frame's phase. On a Ryzen 7 6800U (512 KiB of L2 per core) a 24 MP frame
/// demosaics in 125 ms with tiles of 128, against 129 ms at 96, 127 ms at 160, 182 ms at 256 and
/// 227 ms untiled.
const TILE: usize = 128;

/// The output's three planes, and the peak: the caller's input, the output, and the workers' tile
/// buffers. The workers are the pool's, which demosaics running at once share, so the charge to
/// each is an upper bound.
pub(crate) fn demosaic_memory(size: Size2us) -> DemosaicMemory {
    let plane_bytes = size
        .width
        .saturating_mul(size.height)
        .saturating_mul(size_of::<f32>());
    let output_bytes = plane_bytes.saturating_mul(3);
    DemosaicMemory {
        output_bytes,
        peak_bytes: plane_bytes
            .saturating_add(output_bytes)
            .saturating_add(workspace_bytes()),
    }
}

/// The tile buffers of every worker of the pool, at most one each: a worker makes its tile for a
/// run of tiles and drops it before it takes other work.
pub(crate) fn workspace_bytes() -> usize {
    rayon::current_num_threads().saturating_mul(Tile::bytes())
}

/// Where each tile along an axis of `extent` pixels starts: from 0, every `step`, until one reaches
/// the far edge.
fn tile_starts(extent: usize, step: usize) -> impl Iterator<Item = usize> {
    let count = extent.saturating_sub(TILE).div_ceil(step) + 1;
    (0..count).map(move |index| index * step)
}

/// Linear interpolation: `(1 - a) * b + a * c`.
#[inline(always)]
fn intp(a: f32, b: f32, c: f32) -> f32 {
    b + a * (c - b)
}

#[inline(always)]
fn estimate_green(neighbor_green: f32, center_lpf: f32, same_color_lpf: f32) -> f32 {
    let numerator = neighbor_green * (center_lpf + center_lpf);
    let denominator = EPS + center_lpf + same_color_lpf;
    if center_lpf >= 0.0 && same_color_lpf >= 0.0 {
        return numerator / denominator;
    }

    let scale = EPS + center_lpf.abs() + same_color_lpf.abs();
    let transition = MIN_SIGNED_DENOMINATOR_RATIO * scale;
    if denominator.abs() >= transition {
        return numerator / denominator;
    }

    // Same-color LPFs are two pixels apart; midpoint correction halves their 4× gain.
    let additive = neighbor_green + (center_lpf - same_color_lpf) * 0.125;
    let t = denominator.abs() / transition;
    let curve = t * (3.0 - 2.0 * t);
    let ratio_weight = t * curve;
    // Fold the reciprocal into smoothstep so exact cancellation cannot form 0/0.
    let weighted_ratio = numerator * denominator.signum() * curve / transition;
    additive * (1.0 - ratio_weight) + weighted_ratio
}

/// Mean of the four diagonal neighbours of `idx` (stride `w1`) in the V/H direction map: the
/// discriminator's local average, which pulls a pixel's estimate toward its neighbourhood.
///
/// Summed in pairs, and the P/Q map's in order, as librtprocess sums them: the cross-check holds
/// the output to librtprocess's bits.
#[inline(always)]
fn vh_neighbourhood(vh_dir: &[f32], idx: usize, w1: usize) -> f32 {
    0.25 * ((vh_dir[idx - w1 - 1] + vh_dir[idx - w1 + 1])
        + (vh_dir[idx + w1 - 1] + vh_dir[idx + w1 + 1]))
}

/// [`vh_neighbourhood`] in the P/Q direction map.
#[inline(always)]
fn pq_neighbourhood(pq_dir: &[f32], idx: usize, w1: usize) -> f32 {
    0.25 * (pq_dir[idx - w1 - 1]
        + pq_dir[idx - w1 + 1]
        + pq_dir[idx + w1 - 1]
        + pq_dir[idx + w1 + 1])
}

/// RCD demosaic implementation.
///
/// Input: a Bayer frame, whose calibrated samples may be outside `[0, 1]`.
/// Output: planar RGB f32 channels of its size.
///
/// The frame is demosaiced in tiles of [`TILE`], each run as a frame of its own: from
/// [`INTERPOLATED_BORDER`] in, a crop demosaics bit for bit as inside the frame, so each tile
/// writes that part of itself, and the tiles overlap by twice the border. The pixels nearest the
/// frame's edge come from their neighbours.
pub(crate) fn demosaic(
    bayer: &BayerImage<'_>,
    cancel: &CancelToken,
) -> Result<[Vec<f32>; 3], Cancelled> {
    let Size2us { width, height } = bayer.size;
    let pixels = bayer.size.pixel_count();
    let mut r = vec![0.0f32; pixels];
    let mut g = vec![0.0f32; pixels];
    let mut b = vec![0.0f32; pixels];
    let border = INTERPOLATED_BORDER;
    let step = TILE - 2 * border;
    let places: Vec<TilePlace> = if width > 2 * border && height > 2 * border {
        tile_starts(height, step)
            .flat_map(|top| {
                tile_starts(width, step).map(move |left| TilePlace {
                    top,
                    left,
                    size: Size2us::new((width - left).min(TILE), (height - top).min(TILE)),
                })
            })
            .collect()
    } else {
        Vec::new()
    };
    let out = OutputPlanes {
        r: UnsafeSendPtr::new(r.as_mut_ptr()),
        g: UnsafeSendPtr::new(g.as_mut_ptr()),
        b: UnsafeSendPtr::new(b.as_mut_ptr()),
    };
    places
        .par_iter()
        .try_for_each_init(Tile::new, |tile, &place| {
            Cancelled::check(cancel)?;
            // SAFETY: the planes cover the frame, and the tiles at `step` own disjoint parts.
            unsafe { tile.demosaic(bayer.data, width, bayer.pattern, place, out) };
            Ok(())
        })?;
    Cancelled::check(cancel)?;
    let border = if places.is_empty() {
        width.max(height)
    } else {
        border
    };
    border_interpolate([&mut r, &mut g, &mut b], bayer, border);
    Ok([r, g, b])
}

/// Bilinear interpolation of the pixels within `border` of the frame's edge, row by row in
/// parallel: the whole of each row in the band at the top and bottom, and the ends of each row
/// between.
fn border_interpolate(
    [out_r, out_g, out_b]: [&mut [f32]; 3],
    bayer: &BayerImage<'_>,
    border: usize,
) {
    let Size2us { width, height } = bayer.size;
    out_r
        .par_chunks_mut(width)
        .zip(out_g.par_chunks_mut(width))
        .zip(out_b.par_chunks_mut(width))
        .enumerate()
        .for_each(|(y, ((row_r, row_g), row_b))| {
            let mut fill_span = |span: Range<usize>| {
                for x in span {
                    row_r[x] = bilinear(bayer, 0, x, y);
                    row_g[x] = bilinear(bayer, 1, x, y);
                    row_b[x] = bilinear(bayer, 2, x, y);
                }
            };
            if y < border || y + border >= height || 2 * border >= width {
                fill_span(0..width);
            } else {
                fill_span(0..border);
                fill_span(width - border..width);
            }
        });
}

/// `channel` at `(x, y)`: the pixel's own sample in its colour, and otherwise the mean of that
/// colour's samples among its 8 neighbours, or among its 24 when those hold none.
fn bilinear(bayer: &BayerImage<'_>, channel: usize, x: usize, y: usize) -> f32 {
    let Size2us { width, height } = bayer.size;
    let pattern = bayer.pattern;
    let cfa = bayer.data;
    if pattern.color_at(Vec2us::new(x, y)) == channel {
        return cfa[y * width + x];
    }
    // The pixel `(dy, dx)` away, when it lies inside the frame.
    let neighbour = |dy: isize, dx: isize| {
        y.checked_add_signed(dy)
            .zip(x.checked_add_signed(dx))
            .filter(|&(ny, nx)| ny < height && nx < width)
    };
    let mut sum = 0.0f32;
    let mut count = 0u32;
    for dy in -1isize..=1 {
        for dx in -1isize..=1 {
            if dy == 0 && dx == 0 {
                continue;
            }
            if let Some((ny, nx)) = neighbour(dy, dx)
                && pattern.color_at(Vec2us::new(nx, ny)) == channel
            {
                sum += cfa[ny * width + nx];
                count += 1;
            }
        }
    }
    if count > 0 {
        return sum / count as f32;
    }
    for dy in -2isize..=2 {
        for dx in -2isize..=2 {
            if let Some((ny, nx)) = neighbour(dy, dx)
                && pattern.color_at(Vec2us::new(nx, ny)) == channel
            {
                sum += cfa[ny * width + nx];
                count += 1;
            }
        }
    }
    if count > 0 { sum / count as f32 } else { 0.0 }
}

#[cfg(test)]
mod tests;
