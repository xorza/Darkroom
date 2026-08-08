//! Failures raised while interpreting a RAW file's own metadata, before any pixel is decoded.

/// Why a file's black-level metadata could not be consolidated into a usable normalization range.
///
/// Every variant describes something the *file* claims, not something the code got wrong: libraw
/// hands these values through verbatim from the container, so a truncated or hand-edited RAW can
/// produce any of them.
#[derive(Debug, thiserror::Error)]
pub(super) enum BlackLevelError {
    /// The spatial black pattern's dimensions do not multiply into a `usize`.
    #[error("invalid spatial black pattern dimensions: {width}x{height}")]
    SpatialPatternOverflow { width: u32, height: u32 },

    /// The spatial black pattern claims more entries than the fixed table libraw reports it in.
    #[error("spatial black pattern {width}x{height} exceeds {capacity} entries")]
    SpatialPatternTooLarge {
        width: u32,
        height: u32,
        capacity: usize,
    },

    /// Black is at or above maximum, leaving no range to normalize into.
    #[error("invalid black level: common black {black} >= maximum {maximum}")]
    BlackExceedsMaximum { black: u32, maximum: u32 },
}
