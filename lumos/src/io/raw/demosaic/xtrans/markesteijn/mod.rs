//! Markesteijn demosaicing for X-Trans sensors, one pass or three.
//!
//! Frank Markesteijn's directional interpolation with homogeneity-based direction selection, as
//! dcraw's `xtrans_interpolate` and librtprocess's `markesteijn_demosaic` implement it, in tiles:
//! 1. green in four directions from each non-green pixel's hexagon, held to its bounds;
//! 2. red and blue by Markesteijn's three geometry-specific stages; three passes add four more
//!    directions, whose green is computed again from the interpolated values of nearer pixels.
//!    The stage for 2×2 blocks of green fills every direction: dcraw's fills two of one pass's
//!    four and leaves the others' red and blue at zero (LibRaw issue 441);
//! 3. each direction's ITU-R BT.2020 YPbPr and its second differences along the direction;
//! 4. per direction, how many 3×3 neighbours vary least, summed over 5×5;
//! 5. the mean of the directions within an eighth of the most homogeneous.
//!
//! Every pass uses YPbPr. librtprocess offers CIELab for three passes, through the camera's colour
//! matrix, which a calibrated frame does not carry; the derivatives only choose a direction, and
//! librtprocess calls the two nearly indistinguishable.
//!
//! Each stage reads only what the stage before computed, which lies further from a tile's edge
//! than its own input, so a tile computes its pixels in full only a margin inside its edges.
//! librtprocess's tiles write all but 8 pixels at each side, nearer than that, so its pixels
//! beside a seam depend on where the tiles lie. Here each tile writes only the part its
//! passes compute in full, and the tiles overlap by twice the margin. The pixels nearest the
//! frame's edge, which no tile computes in full, come from their neighbours. The interior is librtprocess's to the bit, run as one tile over the frame, which a test holds
//! it to.

mod border;
mod hex_table;
mod tile;

use common::CancelToken;
use rayon::prelude::*;

use crate::concurrency::unsafe_send_ptr::UnsafeSendPtr;
use crate::io::cancelled::Cancelled;
use crate::io::raw::demosaic::DemosaicMemory;
use crate::io::raw::demosaic::xtrans::XTransImage;
use crate::io::raw::demosaic::xtrans::markesteijn::hex_table::HexTable;
use crate::io::raw::demosaic::xtrans::markesteijn::tile::{Tile, TilePlace};
use crate::math::size2us::Size2us;

/// The side of a tile, where its buffers stay in a core's cache. On a Ryzen 7 6800U (512 KiB of L2
/// per core) a 26 MP frame demosaics in 245 ms at one pass and 703 ms at three with tiles of 96,
/// against 251 and 866 ms at librtprocess's 114 and 431 and 1152 ms at 144; below 84 the overlap
/// each tile computes again costs more than the cache saves.
const TILE: usize = 96;

/// How many passes the X-Trans demosaic makes: four directions, or eight with the green computed
/// again from nearer pixels — LibRaw's default and RawTherapee's best, at about twice the time.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MarkesteijnPasses {
    #[default]
    One,
    Three,
}

impl MarkesteijnPasses {
    const fn count(self) -> usize {
        match self {
            Self::One => 1,
            Self::Three => 3,
        }
    }

    const fn directions(self) -> usize {
        match self {
            Self::One => 4,
            Self::Three => 8,
        }
    }

    /// How far inside its edges a tile computes its pixels in full: the least at which none
    /// reads a colour no stage computed, which a test finds at every phase of the pattern. Each
    /// further pass computes green again from colours 2 to 4 pixels away.
    const fn margin(self) -> usize {
        match self {
            Self::One => 9,
            Self::Three => 15,
        }
    }

    /// The pixels nearest the frame's edge that come from their neighbours: the first tile starts
    /// 3 pixels in, for the reach of the green interpolation, and writes from its margin.
    const fn border(self) -> usize {
        3 + self.margin()
    }
}

/// Where each tile along an axis of `extent` pixels starts: from 3, every `step`, until one reaches
/// 3 pixels from the far edge.
fn tile_starts(extent: usize, step: usize) -> impl Iterator<Item = usize> {
    let count = extent.saturating_sub(6 + TILE).div_ceil(step) + 1;
    (0..count).map(move |index| 3 + index * step)
}

/// The output planes, written by every tile at the pixels it alone owns.
#[derive(Debug, Clone, Copy)]
struct OutputPlanes {
    r: UnsafeSendPtr<f32>,
    g: UnsafeSendPtr<f32>,
    b: UnsafeSendPtr<f32>,
}

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
/// run of tiles and drops it before it takes other work. Charged at eight directions, the most.
pub(crate) fn workspace_bytes() -> usize {
    rayon::current_num_threads().saturating_mul(Tile::bytes(8))
}

/// Demosaic an X-Trans frame with `passes` passes.
///
/// Returns unclipped planar channels `[R, G, B]`, each `width * height`.
pub(crate) fn demosaic(
    xtrans: &XTransImage<'_>,
    passes: MarkesteijnPasses,
    cancel: &CancelToken,
) -> Result<[Vec<f32>; 3], Cancelled> {
    let Size2us { width, height } = xtrans.size;
    let pixels = width * height;
    let mut r = vec![0.0f32; pixels];
    let mut g = vec![0.0f32; pixels];
    let mut b = vec![0.0f32; pixels];
    let hex = HexTable::new(xtrans.pattern, width);
    let border = passes.border();
    let step = TILE - 2 * passes.margin();
    let places: Vec<TilePlace> = if width > 2 * border && height > 2 * border {
        tile_starts(height, step)
            .flat_map(|top| tile_starts(width, step).map(move |left| TilePlace { top, left }))
            .collect()
    } else {
        Vec::new()
    };
    let out = OutputPlanes {
        r: UnsafeSendPtr::new(r.as_mut_ptr()),
        g: UnsafeSendPtr::new(g.as_mut_ptr()),
        b: UnsafeSendPtr::new(b.as_mut_ptr()),
    };
    places.par_iter().try_for_each_init(
        || Tile::new(passes.directions()),
        |tile, &place| {
            Cancelled::check(cancel)?;
            // SAFETY: the planes cover the frame, and the tiles at `step` own disjoint parts.
            unsafe { tile.demosaic(xtrans, &hex, place, passes.count(), passes.margin(), out) };
            Ok(())
        },
    )?;
    Cancelled::check(cancel)?;
    let border = if places.is_empty() {
        width.max(height)
    } else {
        border
    };
    border::fill(xtrans, [&mut r, &mut g, &mut b], border);
    Ok([r, g, b])
}

#[cfg(all(test, feature = "bench"))]
mod bench;
#[cfg(test)]
mod tests;
