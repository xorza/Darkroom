//! The errors of the image types: a record or a pattern that is not one.

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
    /// The demosaic reads greens in a 3×3 cell, so the 6×6 layout must repeat them every three.
    #[error(
        "invalid X-Trans greens: row {row}, column {column} is not green as row {}, column {} is",
        row % 3,
        column % 3
    )]
    GreenPeriod { row: usize, column: usize },
    /// Markesteijn's hexagons are built around a solitary green, a green whose four neighbours are
    /// none of them green, which the layout's 3×3 cell of greens must hold exactly one of. With
    /// greens that repeat every three, a layout has one exactly when every hexagon fills: of the
    /// layouts that pass every other check, an enumeration of them all finds no third kind.
    #[error("invalid X-Trans greens: no solitary green to build the demosaic's hexagons around")]
    Hexagons,
}

/// Why a record is no [`FlatGain`](crate::image::flat_gain::FlatGain): what a damaged kept frame
/// reads as, a miss rather than a gain that covers other pixels.
#[derive(Debug, thiserror::Error)]
#[error("a flat gain record of {channels} channels, {bins} bin sets, or nodes off its grid")]
pub(crate) struct InvalidFlatGain {
    pub(crate) channels: usize,
    pub(crate) bins: usize,
}
