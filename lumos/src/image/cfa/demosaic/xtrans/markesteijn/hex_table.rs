//! [`HexTable`]: the pattern facts the Markesteijn tiles read, in their input's and their own
//! strides.

use crate::image::cfa::cfa_color::CfaColor;
use crate::image::cfa::demosaic::xtrans::markesteijn::{CROP, TILE};
use crate::image::cfa::xtrans_pattern::XTransPattern;
use common::Vec2us;

/// Each `(row % 3, col % 3)`'s hexagon as flat offsets — in the input crop's stride, which the
/// samples are read in, and in the tile's, which its buffers are — beside the pattern's colors and
/// where its solitary green and its rows of two greens lie: librtprocess's `allhex`, `fc`,
/// `isgreen`, `sgrow`/`sgcol` and `RightShift`.
#[derive(Debug)]
pub(super) struct HexTable {
    input: [[[isize; 8]; 3]; 3],
    tile: [[[isize; 8]; 3]; 3],
    pattern: XTransPattern,
    right_shift: [bool; 3],
    sgrow: usize,
    sgcol: usize,
}

impl HexTable {
    pub(super) fn new(pattern: XTransPattern) -> Self {
        let hexagons = pattern.hexagons();
        let flat = |stride: isize| {
            let mut table = [[[0isize; 8]; 3]; 3];
            for (row, cells) in table.iter_mut().enumerate() {
                for (col, cell) in cells.iter_mut().enumerate() {
                    for (entry, offset) in cell.iter_mut().zip(hexagons.at(Vec2us::new(col, row))) {
                        *entry = isize::from(offset.dy) * stride + isize::from(offset.dx);
                    }
                }
            }
            table
        };
        let mut right_shift = [false; 3];
        for (row, shift) in right_shift.iter_mut().enumerate() {
            let greens = (0..3)
                .filter(|&col| pattern.color_at(Vec2us::new(col, row)) == CfaColor::Green)
                .count();
            *shift = greens == 2;
        }
        Self {
            input: flat(CROP as isize),
            tile: flat(TILE as isize),
            pattern,
            right_shift,
            sgrow: hexagons.solitary().y,
            sgcol: hexagons.solitary().x,
        }
    }

    /// The hexagon at `(row, col)` in the input crop's stride.
    #[inline(always)]
    pub(super) const fn input(&self, row: usize, col: usize) -> &[isize; 8] {
        &self.input[row % 3][col % 3]
    }

    /// The hexagon at `(row, col)` in the tile's stride.
    #[inline(always)]
    pub(super) const fn tile(&self, row: usize, col: usize) -> &[isize; 8] {
        &self.tile[row % 3][col % 3]
    }

    /// The color index at `(row, col)`: 0 red, 1 green, 2 blue.
    #[inline(always)]
    pub(super) const fn color(&self, row: usize, col: usize) -> usize {
        self.pattern.color_at(Vec2us::new(col, row)).idx()
    }

    /// Whether `(row, col)` is green; greens repeat every three rows and columns.
    #[inline(always)]
    pub(super) const fn is_green(&self, row: usize, col: usize) -> bool {
        matches!(
            self.pattern.color_at(Vec2us::new(col % 3, row % 3)),
            CfaColor::Green
        )
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
