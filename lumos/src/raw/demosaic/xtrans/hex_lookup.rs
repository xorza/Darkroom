//! Pre-computed hexagonal neighbor lookup tables for Markesteijn demosaicing.
//!
//! The Markesteijn algorithm uses hexagonal neighbor patterns derived from the
//! X-Trans 6×6 CFA layout. The pattern has a 3×3 sub-symmetry, so the hex
//! neighbors repeat every 3 rows and 3 columns.
//!
//! The `allhex[row%3][col%3][set][0..8]` table stores 8 relative offsets per
//! position per set. Set 0 uses image stride for offsets, set 1 uses tile stride.
//! We only need set 0 (full-image processing, no tiles).
//!
//! Reference: Frank Markesteijn's algorithm as implemented in dcraw/libraw.

use super::XTransPattern;

/// Number of hex neighbor entries per pattern position.
const HEX_ENTRIES: usize = 8;

/// Orthogonal direction vectors, cycled for rotation.
/// Each pair (orth[d], orth[d+1]) and (orth[d+2], orth[d+3]) form a
/// direction basis for hex neighbor computation.
const ORTH: [i32; 12] = [1, 0, 0, 1, -1, 0, 0, -1, 1, 0, 0, 1];

/// Pattern coefficients for hex neighbor offset computation.
/// patt[0] is for non-green pixels, patt[1] is for green pixels.
const PATT: [[i32; 16]; 2] = [
    [0, 1, 0, -1, 2, 0, -1, 0, 1, 1, 1, -1, 0, 0, 0, 0],
    [0, 1, 0, -2, 1, 0, -2, 0, 1, 1, -2, -2, 1, -1, -1, 1],
];

/// Pre-computed hexagonal neighbor offsets for Markesteijn algorithm.
///
/// Indexed by `[row % 3][col % 3]`, contains 8 relative (dy, dx) offsets
/// for the hex neighborhood used in green interpolation.
#[derive(Debug)]
pub(super) struct HexLookup {
    /// offsets[row%3][col%3] = [(dy, dx); 8]
    offsets: [[[HexOffset; HEX_ENTRIES]; 3]; 3],
    /// The "solitary green" row position (mod 3)
    pub(super) sgrow: usize,
}

/// A single hex neighbor offset.
#[derive(Debug, Clone, Copy, Default)]
pub(super) struct HexOffset {
    pub dy: i32,
    pub dx: i32,
}

impl HexLookup {
    /// Build the hex lookup table from an X-Trans pattern.
    ///
    /// This replicates libraw's allhex construction: for each (row%3, col%3)
    /// position, it finds the 8 hex neighbor offsets based on the local CFA
    /// pattern geometry. The construction rotates through 4 orthogonal
    /// directions and applies pattern-specific coefficients.
    pub(super) fn new(pattern: &XTransPattern) -> Self {
        let mut offsets = [[[HexOffset::default(); HEX_ENTRIES]; 3]; 3];
        let mut sgrow = 0usize;

        // Sentinel value to detect unfilled entries
        for row_offsets in &mut offsets {
            for col_offsets in row_offsets.iter_mut() {
                for entry in col_offsets.iter_mut() {
                    *entry = HexOffset {
                        dy: i32::MAX,
                        dx: i32::MAX,
                    };
                }
            }
        }

        for (row, row_offsets) in offsets.iter_mut().enumerate() {
            for (col, col_offsets) in row_offsets.iter_mut().enumerate() {
                let mut ng: i32 = 0;
                // Iterate through 5 orthogonal directions (d=0,2,4,6,8)
                // d indexes into ORTH to get direction vectors
                let mut d = 0;
                while d < 10 {
                    let g = if pattern.color_at(row, col) == 1 {
                        1i32
                    } else {
                        0i32
                    };

                    // Check if neighbor in this direction is green
                    // Add 6 before converting to usize so negative offsets wrap correctly
                    let nr = (row as i32 + ORTH[d] + 6) as usize;
                    let nc = (col as i32 + ORTH[d + 2] + 6) as usize;
                    if pattern.color_at(nr, nc) == 1 {
                        ng = 0;
                    } else {
                        ng += 1;
                    }

                    // When we find 4 consecutive non-green neighbors,
                    // this marks the "solitary green" position
                    if ng == 4 {
                        sgrow = row;
                    }

                    // Build hex offsets when we hit the right count
                    // For non-green pixels (g=0): trigger at ng==1
                    // For green pixels (g=1): trigger at ng==2
                    if ng == g + 1 {
                        for c in 0..HEX_ENTRIES {
                            let v = ORTH[d] * PATT[g as usize][c * 2]
                                + ORTH[d + 1] * PATT[g as usize][c * 2 + 1];
                            let h = ORTH[d + 2] * PATT[g as usize][c * 2]
                                + ORTH[d + 3] * PATT[g as usize][c * 2 + 1];

                            // The XOR with (g*2 & d) rotates the entry index
                            // to maintain consistent hex geometry across directions
                            let idx = c ^ ((g * 2) as usize & d);
                            col_offsets[idx] = HexOffset { dy: v, dx: h };
                        }
                    }

                    d += 2;
                }
            }
        }

        // Verify all entries were filled
        for (r, row_offsets) in offsets.iter().enumerate() {
            for (c, col_offsets) in row_offsets.iter().enumerate() {
                for (e, entry) in col_offsets.iter().enumerate() {
                    assert!(
                        entry.dy != i32::MAX,
                        "Unfilled hex entry at [{r}][{c}][{e}]"
                    );
                }
            }
        }

        Self { offsets, sgrow }
    }

    /// Get hex offsets for a given (row, col) position.
    #[inline(always)]
    pub(super) fn get(&self, row: usize, col: usize) -> &[HexOffset; HEX_ENTRIES] {
        &self.offsets[row % 3][col % 3]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_pattern() -> XTransPattern {
        XTransPattern::new([
            [1, 0, 1, 1, 2, 1], // G R G G B G
            [2, 1, 2, 0, 1, 0], // B G B R G R
            [1, 2, 1, 1, 0, 1], // G B G G R G
            [1, 2, 1, 1, 0, 1], // G B G G R G
            [0, 1, 0, 2, 1, 2], // R G R B G B
            [1, 0, 1, 1, 2, 1], // G R G G B G
        ])
    }

    #[test]
    fn test_hex_lookup_construction() {
        let pattern = test_pattern();
        let hex = HexLookup::new(&pattern);

        // All entries should be filled (no sentinel values)
        for r in 0..3 {
            for c in 0..3 {
                for e in 0..HEX_ENTRIES {
                    let off = &hex.offsets[r][c][e];
                    assert!(
                        off.dy != i32::MAX && off.dx != i32::MAX,
                        "Unfilled entry [{r}][{c}][{e}]"
                    );
                }
            }
        }
    }

    #[test]
    fn test_hex_lookup_offsets_in_range() {
        let pattern = test_pattern();
        let hex = HexLookup::new(&pattern);

        // All offsets should be within a reasonable range (±3 pixels)
        for r in 0..3 {
            for c in 0..3 {
                for e in 0..HEX_ENTRIES {
                    let off = &hex.offsets[r][c][e];
                    assert!(
                        off.dy.abs() <= 3 && off.dx.abs() <= 3,
                        "Offset [{r}][{c}][{e}] = ({}, {}) out of range",
                        off.dy,
                        off.dx
                    );
                }
            }
        }
    }

    #[test]
    fn test_hex_lookup_sgrow() {
        let pattern = test_pattern();
        let hex = HexLookup::new(&pattern);

        // sgrow should be within 0..3
        assert!(hex.sgrow < 3);
    }

    #[test]
    fn test_hex_lookup_has_green_neighbors() {
        let pattern = test_pattern();
        let hex = HexLookup::new(&pattern);

        // For non-green pixels, hex neighbors should include green positions
        for r in 0..3 {
            for c in 0..3 {
                if pattern.color_at(r, c) != 1 {
                    let offsets = hex.get(r, c);
                    // First two hex neighbors (indices 0,1) are used for
                    // green interpolation — they should point to green pixels
                    let n0_color = pattern.color_at(
                        (r as i32 + offsets[0].dy) as usize,
                        (c as i32 + offsets[0].dx) as usize,
                    );
                    let n1_color = pattern.color_at(
                        (r as i32 + offsets[1].dy) as usize,
                        (c as i32 + offsets[1].dx) as usize,
                    );
                    assert_eq!(
                        n0_color, 1,
                        "Hex[{r}][{c}][0] at ({},{}) should be green",
                        offsets[0].dy, offsets[0].dx
                    );
                    assert_eq!(
                        n1_color, 1,
                        "Hex[{r}][{c}][1] at ({},{}) should be green",
                        offsets[1].dy, offsets[1].dx
                    );
                }
            }
        }
    }

    #[test]
    fn test_hex_lookup_mod3_wrapping() {
        let pattern = test_pattern();
        let hex = HexLookup::new(&pattern);

        // get() should wrap via % 3
        assert_eq!(
            hex.get(0, 0) as *const _,
            hex.get(3, 3) as *const _,
            "get(0,0) should equal get(3,3)"
        );
        assert_eq!(
            hex.get(1, 2) as *const _,
            hex.get(4, 5) as *const _,
            "get(1,2) should equal get(4,5)"
        );
    }
}
