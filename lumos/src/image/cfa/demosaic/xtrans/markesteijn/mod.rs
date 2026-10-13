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
//! Each stage reads only what the stage before computed, which lies further from a tile's edge than
//! its own input, so a tile computes its pixels in full only a margin inside its edges.
//! librtprocess's tiles write all but 8 pixels at each side, nearer than that, so its pixels beside
//! a seam depend on where the tiles lie. Here each tile writes only the part its passes compute in
//! full, and the tiles overlap by twice the margin. The pixels nearest the frame's edge, which no
//! tile computes in full, come from their neighbours. The interior is librtprocess's to the bit,
//! run as one tile over the frame, which a test holds it to.

mod hex_table;
mod tile;

use common::CancelToken;
use serde::{Deserialize, Serialize};

use crate::error::Cancelled;
use crate::image::cfa::demosaic::tiled;
use crate::image::cfa::demosaic::tiled::{Mosaic, TileKernel, Tiling};
use crate::image::cfa::demosaic::xtrans::markesteijn::hex_table::HexTable;
use crate::image::cfa::demosaic::xtrans::markesteijn::tile::{Tile, TileJob};
use crate::image::cfa::demosaic::xtrans::xtrans_image::XTransImage;
use crate::math::size2us::Size2us;
use crate::memory::footprint::Footprint;

/// The side of a tile, where its buffers stay in a core's cache. On a Ryzen 7 6800U (512 KiB of L2
/// per core) a 26 MP frame demosaics in 245 ms at one pass and 703 ms at three with tiles of 96,
/// against 251 and 866 ms at librtprocess's 114 and 431 and 1152 ms at 144; below 84 the overlap
/// each tile computes again costs more than the cache saves.
const TILE: usize = 96;
/// How far beyond its own pixels a tile reads the frame: the reach of the green interpolation.
const READ_REACH: usize = 3;
/// The side of a tile's input: the tile and [`READ_REACH`] each side.
const CROP: usize = TILE + 2 * READ_REACH;

/// How many passes the X-Trans demosaic makes: four directions, or eight with the green computed
/// again from nearer pixels — LibRaw's default and RawTherapee's best, at about twice the time.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
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

    /// How the passes tile a frame: tiles start [`READ_REACH`] inside the frame, for the reach of
    /// the green interpolation, and write from their margin.
    const fn tiling(self) -> Tiling {
        Tiling {
            tile: TILE,
            inset: READ_REACH,
            margin: self.margin(),
        }
    }
}

/// What a demosaic of a frame of `size` holds on `workers` — see [`tiled::footprint`] — charged
/// at eight directions, the most.
pub(crate) const fn footprint(size: Size2us, workers: usize) -> Footprint {
    tiled::footprint(size, Tile::bytes(8), workers)
}

/// Demosaic an X-Trans frame with `passes` passes, its colours balanced by its gains.
///
/// Returns unclipped planar channels `[R, G, B]`, each `width * height`, in the frame's own
/// balance.
pub(crate) fn demosaic(
    xtrans: &XTransImage<'_>,
    passes: MarkesteijnPasses,
    cancel: &CancelToken,
) -> Result<[Vec<f32>; 3], Cancelled> {
    let hex = HexTable::new(xtrans.pattern);
    let mosaic = Mosaic {
        data: xtrans.data,
        size: xtrans.size,
        color_at: |position| xtrans.pattern.color_at(position).index(),
    };
    let kernel = TileKernel {
        tiling: passes.tiling(),
        new_tile: || Tile::new(passes.directions()),
        run: |tile: &mut Tile, place, out| unsafe {
            let job = TileJob {
                xtrans,
                hex: &hex,
                passes,
            };
            tile.demosaic(job, place, out);
        },
    };
    mosaic.demosaic(kernel, cancel)
}

#[cfg(test)]
pub(crate) mod internals {
    use crate::image::cfa::demosaic::tiled;
    use crate::image::cfa::demosaic::xtrans::markesteijn::MarkesteijnPasses;
    use crate::image::cfa::demosaic::xtrans::markesteijn::tile::Tile;

    impl MarkesteijnPasses {
        /// The pixels nearest the frame's edge that come from their neighbours.
        pub(crate) const fn border(self) -> usize {
            self.tiling().border()
        }
    }

    /// The tile buffers of `workers` — see [`tiled::workspace_bytes`] — charged at eight
    /// directions, the most.
    pub(crate) fn workspace_bytes(workers: usize) -> usize {
        tiled::workspace_bytes(Tile::bytes(8), workers)
    }
}

#[cfg(test)]
mod tests;
