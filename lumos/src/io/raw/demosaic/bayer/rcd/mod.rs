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

use common::CancelToken;

use crate::io::cancelled::Cancelled;
use crate::io::raw::demosaic::DemosaicMemory;
use crate::io::raw::demosaic::bayer::BayerImage;
use crate::io::raw::demosaic::bayer::rcd::tile::Tile;
use crate::io::raw::demosaic::tiled;
use crate::io::raw::demosaic::tiled::Tiling;
use crate::math::size2us::Size2us;

/// Keeps an inverse-gradient weight finite where a gradient is zero, as librtprocess's `eps`.
const EPS: f32 = 1e-5;
const EPSSQ: f32 = 1e-10;
/// The share of `|c| + |s|` below which the ratio's denominator `c + s` hands over to the additive
/// estimate: the ratio's relative condition number stays at most `1/0.25 = 4`.
const MIN_SIGNED_DENOMINATOR_RATIO: f32 = 0.25;
/// Border size required by the algorithm (pixels on each side).
const BORDER: usize = 4;
/// The band the border fill takes from neighbours (see [`tiled`]). RCD's stages chain stencils — the direction maps and
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

/// The memory a demosaic of a frame of `size` holds — see [`tiled::demosaic_memory`].
pub(crate) fn demosaic_memory(size: Size2us) -> DemosaicMemory {
    tiled::demosaic_memory(size, Tile::bytes())
}

/// Linear interpolation: `(1 - a) * b + a * c`.
#[inline(always)]
fn intp(a: f32, b: f32, c: f32) -> f32 {
    b + a * (c - b)
}

/// The green at a red or blue site from a neighbouring green `g`, as RCD's ratio
/// `g·2c/(c + s)` of the site's low-pass value `c` and its same-colour neighbour's `s`.
///
/// The ratio is level-free: librtprocess's `eps` in the denominator would bias faint signal by
/// `eps/(c + s)`, a colour cast in a dark-subtracted background. Where `c + s` nears cancellation
/// against `|c| + |s|`, only possible on signed data, it blends into the additive midpoint
/// estimate, which it equals at `c = s = 0`.
#[inline(always)]
fn estimate_green(neighbor_green: f32, center_lpf: f32, same_color_lpf: f32) -> f32 {
    let scale = center_lpf.abs() + same_color_lpf.abs();
    if scale == 0.0 {
        return neighbor_green;
    }
    let numerator = neighbor_green * (center_lpf + center_lpf);
    let denominator = center_lpf + same_color_lpf;
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
/// Input: a Bayer frame, whose calibrated samples may be outside `[0, 1]`, and the gains that
/// balance its colours.
/// Output: planar RGB f32 channels of its size, in the frame's own balance.
///
/// The frame is demosaiced in tiles of [`TILE`], each run as a frame of its own: from
/// [`INTERPOLATED_BORDER`] in, a crop demosaics bit for bit as inside the frame, so each tile
/// writes that part of itself, and the tiles overlap by twice the border. The pixels nearest the
/// frame's edge come from their neighbours.
pub(crate) fn demosaic(
    bayer: &BayerImage<'_>,
    cancel: &CancelToken,
) -> Result<[Vec<f32>; 3], Cancelled> {
    let tiling = Tiling {
        tile: TILE,
        inset: 0,
        margin: INTERPOLATED_BORDER,
    };
    tiled::demosaic(
        bayer.data,
        bayer.size,
        |position| bayer.pattern.color_at(position),
        tiling,
        Tile::new,
        // SAFETY: the driver hands each place's own part to this tile alone, which writes no
        // other.
        |tile, place, out| unsafe { tile.demosaic(bayer, place, out) },
        cancel,
    )
}

#[cfg(test)]
pub(crate) mod internals {
    use crate::io::raw::demosaic::bayer::rcd::tile::Tile;
    use crate::io::raw::demosaic::tiled;

    /// The tile buffers of every worker of the pool — see [`tiled::workspace_bytes`].
    pub(crate) fn workspace_bytes() -> usize {
        tiled::workspace_bytes(Tile::bytes())
    }
}

#[cfg(test)]
mod tests;
