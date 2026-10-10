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
use crate::simd::{F32x8, Isa, Mask8};

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

/// The side of a tile, where its phase planes stay in a core's cache, and even, so that every
/// tile starts on the frame's phase. On a Ryzen 7 6800U (512 KiB of L2 per core), built for plain
/// x86-64, a 24 MP frame demosaics in 66 ms with tiles of 128, against 66 ms at 96, 65 ms at 112,
/// 67 ms at 144, 69 ms at 160, 77 ms at 192 and 105 ms at 256.
const TILE: usize = 128;

/// The memory a demosaic of a frame of `size` holds — see [`tiled::demosaic_memory`].
pub(crate) fn demosaic_memory(size: Size2us) -> DemosaicMemory {
    tiled::demosaic_memory(size, Tile::bytes())
}

/// Linear interpolation: `(1 - a) * b + a * c`.
#[inline(always)]
fn intp<V: F32x8>(a: V, b: V, c: V) -> V {
    b + a * (c - b)
}

/// The green at red or blue sites from a neighbouring green `g`, as RCD's ratio `g·2c/(c + s)`
/// of each site's low-pass value `c` and its same-colour neighbour's `s`.
///
/// The ratio is level-free: librtprocess's `eps` in the denominator would bias faint signal by
/// `eps/(c + s)`, a colour cast in a dark-subtracted background. Where `c + s` nears cancellation
/// against `|c| + |s|`, only possible on signed data, it blends into the additive midpoint
/// estimate, which it equals at `c = s = 0`.
///
/// Every lane computes each case and keeps its own, so a lane another case divides by zero in
/// never reaches the output.
#[inline(always)]
fn estimate_green<S: Isa>(
    isa: S,
    neighbor_green: S::F32,
    center_lpf: S::F32,
    same_color_lpf: S::F32,
) -> S::F32 {
    let one = isa.splat_f32(1.0);
    let scale = center_lpf.abs() + same_color_lpf.abs();
    let numerator = neighbor_green * (center_lpf + center_lpf);
    let denominator = center_lpf + same_color_lpf;
    let transition = isa.splat_f32(MIN_SIGNED_DENOMINATOR_RATIO) * scale;
    let ratio = numerator / denominator;

    // Same-color LPFs are two pixels apart; midpoint correction halves their 4× gain.
    let additive = neighbor_green + (center_lpf - same_color_lpf) * isa.splat_f32(0.125);
    let t = denominator.abs() / transition;
    let curve = t * (isa.splat_f32(3.0) - isa.splat_f32(2.0) * t);
    let ratio_weight = t * curve;
    // `f32::signum`, whose −0 is −1: 1/x has the sign of x, a zero's included.
    let signum = (one / denominator)
        .lanes_lt(isa.splat_f32(0.0))
        .select(isa.splat_f32(-1.0), one);
    // Fold the reciprocal into smoothstep so exact cancellation cannot form 0/0.
    let weighted_ratio = numerator * signum * curve / transition;
    let blended = additive * (one - ratio_weight) + weighted_ratio;

    let estimate = denominator
        .abs()
        .lanes_lt(transition)
        .select(blended, ratio);
    scale
        .lanes_eq(isa.splat_f32(0.0))
        .select(neighbor_green, estimate)
}

/// A direction map at a site whose own value is `central` and whose diagonal neighbours' mean is
/// `neighbourhood`: the neighbourhood where it is the further from ½, which pulls the site's
/// estimate toward its neighbours'.
#[inline(always)]
fn discriminate<S: Isa>(isa: S, central: S::F32, neighbourhood: S::F32) -> S::F32 {
    let half = isa.splat_f32(0.5);
    (half - central)
        .abs()
        .lanes_lt((half - neighbourhood).abs())
        .select(neighbourhood, central)
}

/// Mean of a site's four diagonal neighbours `[nw, ne, sw, se]` in the V/H direction map.
///
/// Summed in pairs, and the P/Q map's in order, as librtprocess sums them: the cross-check holds
/// the output to librtprocess's bits.
#[inline(always)]
fn vh_neighbourhood<S: Isa>(isa: S, [nw, ne, sw, se]: [S::F32; 4]) -> S::F32 {
    isa.splat_f32(0.25) * ((nw + ne) + (sw + se))
}

/// [`vh_neighbourhood`] in the P/Q direction map.
#[inline(always)]
fn pq_neighbourhood<S: Isa>(isa: S, [nw, ne, sw, se]: [S::F32; 4]) -> S::F32 {
    isa.splat_f32(0.25) * (nw + ne + sw + se)
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
        || Tile::new(bayer.pattern),
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
