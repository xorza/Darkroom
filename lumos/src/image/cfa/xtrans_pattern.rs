//! [`XTransPattern`]: a checked 6×6 X-Trans color layout.

use serde::{Deserialize, Deserializer, Serialize, Serializer, de};

use crate::image::cfa::cfa_color::CfaColor;
use crate::image::cfa::demosaic::mosaic_view::MosaicPattern;
use crate::image::error::XTransPatternError;
use crate::math::vec2us::Vec2us;

/// A Fujifilm X-Trans color filter layout: 6×6 color indices (0 = red, 1 = green, 2 = blue),
/// the color at `(x, y)` the index at row `y % 6` and column `x % 6`.
///
/// Only a layout the demosaic can work on exists: every value is a color, the counts are the
/// X-Trans 8 red, 20 green and 8 blue, every green has as many red as blue neighbours, the greens
/// repeat every three rows and columns, and their 3×3 cell holds the solitary green Markesteijn's
/// hexagons are built around. Deserializing checks the same, so a stored pattern cannot bring
/// an invalid one back.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct XTransPattern {
    colors: [[CfaColor; 6]; 6],
}

/// Markesteijn's hexagons over a pattern's 3×3 cell of greens, as dcraw's `xtrans_interpolate`
/// builds them (`allhex`, `sgrow`, `sgcol`): for each `(x % 3, y % 3)`, the eight
/// `(dy, dx)` offsets its interpolation reads, and the cell's solitary green, the one whose four
/// neighbours are none of them green.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Hexagons {
    offsets: [[[HexOffset; 8]; 3]; 3],
    solitary: Vec2us,
}

/// One hexagon neighbour's offset from its pixel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct HexOffset {
    pub(crate) dy: i8,
    pub(crate) dx: i8,
}

impl Hexagons {
    /// The eight offsets of the hexagon at `pos`.
    #[inline(always)]
    pub(crate) const fn at(&self, pos: Vec2us) -> &[HexOffset; 8] {
        &self.offsets[pos.y % 3][pos.x % 3]
    }

    /// Where the cell's solitary green lies, within the first 3×3 cell.
    pub(crate) const fn solitary(&self) -> Vec2us {
        self.solitary
    }
}

/// Unit steps cycled through four directions, dcraw's `orth`: direction `d` (even) steps
/// `(orth[d], orth[d + 2])` and its basis is `(orth[d], orth[d + 1])` down, `(orth[d + 2],
/// orth[d + 3])` across.
const ORTH: [i8; 12] = [1, 0, 0, 1, -1, 0, 0, -1, 1, 0, 0, 1];

/// dcraw's `patt`: each hexagon's eight neighbours in a direction's basis, for a pixel that is not
/// green (`[0]`) and one that is (`[1]`).
const PATT: [[i8; 16]; 2] = [
    [0, 1, 0, -1, 2, 0, -1, 0, 1, 1, 1, -1, 0, 0, 0, 0],
    [0, 1, 0, -2, 1, 0, -2, 0, 1, 1, -2, -2, 1, -1, -1, 1],
];

impl MosaicPattern for XTransPattern {
    #[inline(always)]
    fn color_at(&self, pos: Vec2us) -> CfaColor {
        self.color_at(pos)
    }
}

impl XTransPattern {
    /// Check `rows` as an X-Trans layout.
    pub const fn new(rows: [[u8; 6]; 6]) -> Result<Self, XTransPatternError> {
        let mut counts = [0usize; 3];
        let mut row = 0;
        while row < 6 {
            let mut column = 0;
            while column < 6 {
                let value = rows[row][column];
                if value > 2 {
                    return Err(XTransPatternError::Value { row, column, value });
                }
                counts[value as usize] += 1;
                column += 1;
            }
            row += 1;
        }
        if counts[0] != 8 || counts[1] != 20 || counts[2] != 8 {
            return Err(XTransPatternError::ColorCounts { actual: counts });
        }
        let steps = [(0, 1), (1, 0), (0, 5), (5, 0)];
        let mut row = 0;
        while row < 6 {
            let mut column = 0;
            while column < 6 {
                if rows[row][column] == 1 {
                    let mut neighbors = [0usize; 3];
                    let mut step = 0;
                    while step < steps.len() {
                        // Five steps forward is one back, modulo the 6-pixel period.
                        let (dy, dx) = steps[step];
                        neighbors[rows[(row + dy) % 6][(column + dx) % 6] as usize] += 1;
                        step += 1;
                    }
                    if neighbors[0] != neighbors[2] {
                        return Err(XTransPatternError::GreenNeighborhood {
                            row,
                            column,
                            neighbors,
                        });
                    }
                }
                column += 1;
            }
            row += 1;
        }
        let mut row = 0;
        while row < 6 {
            let mut column = 0;
            while column < 6 {
                if (rows[row][column] == 1) != (rows[row % 3][column % 3] == 1) {
                    return Err(XTransPatternError::GreenPeriod { row, column });
                }
                column += 1;
            }
            row += 1;
        }
        if hexagons(&rows).is_none() {
            return Err(XTransPatternError::Hexagons);
        }
        let mut colors = [[CfaColor::Green; 6]; 6];
        let mut row = 0;
        while row < 6 {
            let mut column = 0;
            while column < 6 {
                colors[row][column] = match CfaColor::from_idx(rows[row][column] as usize) {
                    Some(color) => color,
                    None => unreachable!(),
                };
                column += 1;
            }
            row += 1;
        }
        Ok(Self { colors })
    }

    /// Markesteijn's hexagons over the layout, built by the function [`Self::new`] checks them
    /// with.
    pub(crate) const fn hexagons(&self) -> Hexagons {
        hexagons(&self.rows()).expect("a pattern's hexagons were checked when it was made")
    }

    /// The color indices, `[row][column]` over one 6×6 period.
    pub const fn rows(&self) -> [[u8; 6]; 6] {
        let mut rows = [[0; 6]; 6];
        let mut row = 0;
        while row < 6 {
            let mut column = 0;
            while column < 6 {
                rows[row][column] = self.colors[row][column].idx() as u8;
                column += 1;
            }
            row += 1;
        }
        rows
    }

    /// The color at `pos`.
    #[inline(always)]
    pub const fn color_at(&self, pos: Vec2us) -> CfaColor {
        self.colors[pos.y % 6][pos.x % 6]
    }
}

/// dcraw's `allhex` construction: each pixel of the 3×3 cell walks its four neighbours, cycling to
/// a fifth, counting the run of neighbours that are not green; its hexagon is laid out along the
/// direction where that run first reaches one past whether the pixel itself is green, and a run of
/// four marks the solitary green. `None` when an entry stays unset or the cell holds other than one
/// solitary green.
const fn hexagons(rows: &[[u8; 6]; 6]) -> Option<Hexagons> {
    let mut offsets = [[[HexOffset { dy: 0, dx: 0 }; 8]; 3]; 3];
    let mut set = [[[false; 8]; 3]; 3];
    let mut solitary = None;
    let mut solitary_count = 0;
    let mut row = 0;
    while row < 3 {
        let mut column = 0;
        while column < 3 {
            let g = (rows[row][column] == 1) as usize;
            let mut run = 0;
            let mut d = 0;
            while d < 10 {
                // From `row + 6`, a unit step back stays in range and reads the same color.
                let neighbour_row = (row + 6).wrapping_add_signed(ORTH[d] as isize);
                let neighbour_column = (column + 6).wrapping_add_signed(ORTH[d + 2] as isize);
                if rows[neighbour_row % 6][neighbour_column % 6] == 1 {
                    run = 0;
                } else {
                    run += 1;
                }
                if run == 4 {
                    solitary = Some(Vec2us::new(column, row));
                    solitary_count += 1;
                }
                if run == g + 1 {
                    let mut c = 0;
                    while c < 8 {
                        let entry = c ^ ((g * 2) & d);
                        offsets[row][column][entry] = HexOffset {
                            dy: ORTH[d] * PATT[g][c * 2] + ORTH[d + 1] * PATT[g][c * 2 + 1],
                            dx: ORTH[d + 2] * PATT[g][c * 2] + ORTH[d + 3] * PATT[g][c * 2 + 1],
                        };
                        set[row][column][entry] = true;
                        c += 1;
                    }
                }
                d += 2;
            }
            let mut entry = 0;
            while entry < 8 {
                if !set[row][column][entry] {
                    return None;
                }
                entry += 1;
            }
            column += 1;
        }
        row += 1;
    }
    match solitary {
        Some(solitary) if solitary_count == 1 => Some(Hexagons { offsets, solitary }),
        _ => None,
    }
}

/// As its color indices, [`XTransPattern::rows`].
impl Serialize for XTransPattern {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.rows().serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for XTransPattern {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let rows = <[[u8; 6]; 6]>::deserialize(deserializer)?;
        Self::new(rows).map_err(de::Error::custom)
    }
}
