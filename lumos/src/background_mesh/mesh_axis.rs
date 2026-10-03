//! How a tile mesh cuts one image axis.

/// One axis of a tile mesh: tiles of `tile_size` from the origin, the last one cut short where the
/// axis ends — SExtractor's `BACK_SIZE` rule. A tile's centre is the mean index of the pixels it
/// holds, `(start + end − 1)/2`.
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
        self.extent.div_ceil(self.tile_size)
    }

    /// The first pixel of tile `tile`.
    pub(crate) const fn start(self, tile: usize) -> usize {
        tile * self.tile_size
    }

    /// One past the last pixel of tile `tile`.
    pub(crate) const fn end(self, tile: usize) -> usize {
        let end = self.start(tile) + self.tile_size;
        if end < self.extent { end } else { self.extent }
    }

    /// The mean index of the pixels tile `tile` holds.
    pub(crate) fn centre(self, tile: usize) -> f32 {
        (self.start(tile) + self.end(tile) - 1) as f32 * 0.5
    }
}

#[cfg(test)]
mod tests {
    use crate::background_mesh::mesh_axis::MeshAxis;

    /// 100 pixels in 32s: three whole tiles and a remainder of 4, centred at 15.5 + 32k and at
    /// (96 + 99)/2; one pixel is one tile centred on it.
    #[test]
    fn tiles_run_from_the_origin_and_the_last_is_cut_short() {
        let axis = MeshAxis::new(100, 32);
        assert_eq!(axis.count(), 4);
        assert_eq!(
            (0..4)
                .map(|t| (axis.start(t), axis.end(t)))
                .collect::<Vec<_>>(),
            [(0, 32), (32, 64), (64, 96), (96, 100)]
        );
        assert_eq!(
            (0..4).map(|t| axis.centre(t)).collect::<Vec<_>>(),
            [15.5, 47.5, 79.5, 97.5]
        );
        let single = MeshAxis::new(1, 64);
        assert_eq!(
            (single.count(), single.end(0), single.centre(0)),
            (1, 1, 0.0)
        );
    }
}
