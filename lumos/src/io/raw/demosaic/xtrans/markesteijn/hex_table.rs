//! [`HexTable`]: the pattern facts the Markesteijn tiles read, in the frame's and the tile's
//! strides.

use crate::io::raw::demosaic::xtrans::hex_lookup::HexLookup;
use crate::io::raw::demosaic::xtrans::markesteijn::TILE;
use crate::io::raw::demosaic::xtrans::xtrans_pattern::XTransPattern;
use crate::math::vec2us::Vec2us;

/// Each `(row % 3, col % 3)`'s hexagon as flat offsets — in the frame's stride, which the raw
/// samples are read in, and in the tile's, which its buffers are — beside the pattern's colours and
/// where its solitary green and its rows of two greens lie: librtprocess's `allhex`, `fc`,
/// `isgreen`, `sgrow`/`sgcol` and `RightShift`.
#[derive(Debug)]
pub(super) struct HexTable {
    image: [[[isize; 8]; 3]; 3],
    tile: [[[isize; 8]; 3]; 3],
    pattern: XTransPattern,
    right_shift: [bool; 3],
    sgrow: usize,
    sgcol: usize,
}

impl HexTable {
    pub(super) fn new(pattern: XTransPattern, width: usize) -> Self {
        let lookup = HexLookup::new(&pattern);
        let flat = |stride: isize| {
            let mut table = [[[0isize; 8]; 3]; 3];
            for (row, cells) in table.iter_mut().enumerate() {
                for (col, cell) in cells.iter_mut().enumerate() {
                    for (entry, offset) in cell.iter_mut().zip(lookup.get(row, col)) {
                        *entry = offset.dy * stride + offset.dx;
                    }
                }
            }
            table
        };
        let width = isize::try_from(width).expect("a frame narrower than isize::MAX");
        let mut right_shift = [false; 3];
        for (row, shift) in right_shift.iter_mut().enumerate() {
            let greens = (0..3)
                .filter(|&col| pattern.color_at(Vec2us::new(col, row)) == 1)
                .count();
            *shift = greens == 2;
        }
        Self {
            image: flat(width),
            tile: flat(TILE as isize),
            pattern,
            right_shift,
            sgrow: lookup.sgrow,
            sgcol: lookup.sgcol,
        }
    }

    /// The hexagon at `(row, col)` in the frame's stride.
    #[inline(always)]
    pub(super) const fn image(&self, row: usize, col: usize) -> &[isize; 8] {
        &self.image[row % 3][col % 3]
    }

    /// The hexagon at `(row, col)` in the tile's stride.
    #[inline(always)]
    pub(super) const fn tile(&self, row: usize, col: usize) -> &[isize; 8] {
        &self.tile[row % 3][col % 3]
    }

    /// The colour at `(row, col)`: 0 red, 1 green, 2 blue.
    #[inline(always)]
    pub(super) const fn colour(&self, row: usize, col: usize) -> u8 {
        self.pattern.color_at(Vec2us::new(col, row))
    }

    /// Whether `(row, col)` is green; greens repeat every three rows and columns.
    #[inline(always)]
    pub(super) const fn is_green(&self, row: usize, col: usize) -> bool {
        self.pattern.color_at(Vec2us::new(col % 3, row % 3)) == 1
    }

    /// Whether row `row` holds two greens in three columns, and its non-green pixels lie three
    /// apart.
    #[inline(always)]
    pub(super) const fn right_shift(&self, row: usize) -> bool {
        self.right_shift[row % 3]
    }

    pub(super) const fn sgrow(&self) -> usize {
        self.sgrow
    }

    pub(super) const fn sgcol(&self) -> usize {
        self.sgcol
    }
}
