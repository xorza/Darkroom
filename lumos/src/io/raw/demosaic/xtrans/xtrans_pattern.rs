//! [`XTransPattern`]: a checked 6×6 X-Trans colour layout.

use serde::de;
use serde::{Deserialize, Deserializer, Serialize};

use crate::math::vec2us::Vec2us;

/// A Fujifilm X-Trans colour filter layout: 6×6 colour indices (0 = red, 1 = green, 2 = blue),
/// indexed `[row % 6][column % 6]`.
///
/// Only a layout the demosaic can work on exists: every value is a colour, the counts are the
/// X-Trans 8 red, 20 green and 8 blue, and every green has as many red as blue neighbours.
/// Deserializing checks the same, so a stored pattern cannot bring an invalid one back.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct XTransPattern {
    rows: [[u8; 6]; 6],
}

/// Why a 6×6 array is not an X-Trans layout.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum XTransPatternError {
    #[error(
        "invalid X-Trans pattern value {value} at row {row}, column {column}; expected 0, 1, or 2"
    )]
    Value {
        row: usize,
        column: usize,
        value: u8,
    },
    #[error("invalid X-Trans color counts: expected [8, 20, 8], got {actual:?}")]
    ColorCounts { actual: [usize; 3] },
    #[error("invalid X-Trans green neighborhood at row {row}, column {column}: {neighbors:?}")]
    GreenNeighborhood {
        row: usize,
        column: usize,
        neighbors: [usize; 3],
    },
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
        Ok(Self { rows })
    }

    /// The colour indices, `[row][column]` over one 6×6 period.
    pub const fn rows(&self) -> &[[u8; 6]; 6] {
        &self.rows
    }

    /// The colour at `pos`: 0 = red, 1 = green, 2 = blue.
    #[inline(always)]
    pub const fn color_at(&self, pos: Vec2us) -> u8 {
        self.rows[pos.y % 6][pos.x % 6]
    }
}

impl<'de> Deserialize<'de> for XTransPattern {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let rows = <[[u8; 6]; 6]>::deserialize(deserializer)?;
        Self::new(rows).map_err(de::Error::custom)
    }
}
