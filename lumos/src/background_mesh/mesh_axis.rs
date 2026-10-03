//! How a tile mesh cuts one image axis.

/// One axis of a tile mesh: tiles of `tile_size` from the origin, the last one ending where the axis
/// ends. A remainder of at least half a tile is a tile of its own, cut short as SExtractor's
/// `BACK_SIZE` rule cuts it; a narrower one joins the tile before it, since a sliver of a few
/// pixels measures its sky from too few of them, and on a mosaic may hold no pixel of a colour. A
/// tile's centre is the mean index of the pixels it holds, `(start + end − 1)/2`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct MeshAxis {
    extent: usize,
    tile_size: usize,
}

impl MeshAxis {
    /// `extent` pixels in tiles of `tile_size`, both non-zero.
    pub(crate) const fn new(extent: usize, tile_size: usize) -> Self {
        debug_assert!(
            extent > 0 && tile_size > 0,
            "a mesh axis needs pixels and a tile size"
        );
        Self { extent, tile_size }
    }

    /// How many tiles cover the axis.
    pub(crate) const fn count(self) -> usize {
        let whole = self.extent / self.tile_size;
        let remainder = self.extent % self.tile_size;
        if whole == 0 || 2 * remainder >= self.tile_size {
            whole + 1
        } else {
            whole
        }
    }

    /// The first pixel of tile `tile`.
    pub(crate) const fn start(self, tile: usize) -> usize {
        tile * self.tile_size
    }

    /// One past the last pixel of tile `tile`.
    pub(crate) const fn end(self, tile: usize) -> usize {
        if tile + 1 == self.count() {
            self.extent
        } else {
            self.start(tile) + self.tile_size
        }
    }

    /// The mean index of the pixels tile `tile` holds.
    pub(crate) fn centre(self, tile: usize) -> f32 {
        (self.start(tile) + self.end(tile) - 1) as f32 * 0.5
    }
}

#[cfg(test)]
mod tests {
    use crate::background_mesh::mesh_axis::MeshAxis;

    /// 112 pixels in 32s: three whole tiles and a remainder of 16, half a tile, which stands as a
    /// tile cut short, centred at (96 + 111)/2. 100 pixels leave a remainder of 4, which joins the
    /// third tile: 64..100, centred at 81.5. One pixel is one tile centred on it.
    #[test]
    fn a_sliver_joins_the_tile_before_it() {
        let axis = MeshAxis::new(112, 32);
        assert_eq!(axis.count(), 4);
        assert_eq!(
            (0..4)
                .map(|t| (axis.start(t), axis.end(t)))
                .collect::<Vec<_>>(),
            [(0, 32), (32, 64), (64, 96), (96, 112)]
        );
        assert_eq!(
            (0..4).map(|t| axis.centre(t)).collect::<Vec<_>>(),
            [15.5, 47.5, 79.5, 103.5]
        );
        let merged = MeshAxis::new(100, 32);
        assert_eq!(merged.count(), 3);
        assert_eq!(
            (merged.start(2), merged.end(2), merged.centre(2)),
            (64, 100, 81.5)
        );
        let single = MeshAxis::new(1, 64);
        assert_eq!(
            (single.count(), single.end(0), single.centre(0)),
            (1, 1, 0.0)
        );
    }
}
