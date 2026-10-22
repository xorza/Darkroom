//! The errors of the image types: a record or a pattern that is not one, and a frame of another
//! shape than its set's.

use thiserror::Error;

use crate::image::image_dimensions::ImageDimensions;

/// Why a 6×6 array is not an X-Trans layout.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum XTransPatternError {
    #[error(
        "invalid X-Trans pattern value {value} at row {row}, column {col}; expected 0, 1, or 2"
    )]
    Value { row: usize, col: usize, value: u8 },
    #[error("invalid X-Trans color counts: expected [8, 20, 8], got {actual:?}")]
    ColorCounts { actual: [usize; 3] },
    #[error("invalid X-Trans green neighborhood at row {row}, column {col}: {neighbors:?}")]
    GreenNeighborhood {
        row: usize,
        col: usize,
        neighbors: [usize; 3],
    },
    /// The demosaic reads greens in a 3×3 cell, so the 6×6 layout must repeat them every three.
    #[error(
        "invalid X-Trans greens: row {row}, column {col} is not green as row {}, column {} is",
        row % 3,
        col % 3
    )]
    GreenPeriod { row: usize, col: usize },
    /// Markesteijn's hexagons are built around a solitary green, a green whose four neighbours are
    /// none of them green, which the layout's 3×3 cell of greens must hold exactly one of. With
    /// greens that repeat every three, a layout has one exactly when every hexagon fills: of the
    /// layouts that pass every other check, an enumeration of them all finds no third kind.
    #[error("invalid X-Trans greens: no solitary green to build the demosaic's hexagons around")]
    Hexagons,
}

/// The rule an image's dimensions break, the first in the order they are checked.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum DimensionsError {
    #[error("width must be positive")]
    ZeroWidth,
    #[error("height must be positive")]
    ZeroHeight,
    #[error("{width}x{height} has a side past MAX_SIDE")]
    SidePastMax { width: usize, height: usize },
    #[error("only 1 (grayscale) or 3 (RGB) channels supported, got {count}")]
    Channels { count: usize },
    #[error("{width}x{height}x{channels} samples overflow usize")]
    SampleCount {
        width: usize,
        height: usize,
        channels: usize,
    },
}

/// Why a record is no [`FlatGain`](crate::image::flat_gain::FlatGain): what a damaged kept frame
/// reads as, a miss rather than a gain that covers other pixels.
#[derive(Debug, thiserror::Error)]
#[error("a flat gain record of {channels} channels, {bins} bin sets, or nodes off its grid")]
pub(crate) struct InvalidFlatGain {
    pub(crate) channels: usize,
    pub(crate) bins: usize,
}

/// A frame whose pixel grid disagrees with the one the run was set up for.
///
/// Loading, alignment, the combine cache and drizzle accumulation all reject a frame for the same
/// three reasons — which frame, the geometry the run expects, what the frame turned out to be — so
/// they report it with this one type instead of each subsystem's error declaring the variant again.
/// The mismatches that genuinely aren't this keep a typed variant on their module's error: a stored
/// plane knows only its length, a pixel-weight map has no channels, and a calibration master is
/// measured against the sensor rather than against frame 0.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
#[error("frame {idx} is {actual}, expected {expected}")]
pub struct FrameDimensionMismatch {
    /// Position of the offending frame in the input set.
    pub idx: usize,
    /// The geometry every frame has to match — frame 0's, or the accumulator's.
    pub expected: ImageDimensions,
    /// What this frame turned out to be.
    pub actual: ImageDimensions,
}

impl FrameDimensionMismatch {
    /// The error with each frame index it holds, a position in the list it was raised over, read
    /// through `inputs` into the caller's own numbering of those frames.
    pub(crate) const fn renumbered(self, inputs: &[usize]) -> Self {
        Self {
            idx: inputs[self.idx],
            ..self
        }
    }

    /// `Ok(())` when `actual` matches, else the rejection of frame `idx`: the Result-returning
    /// counterpart to `assert_eq!` for frame geometry, in
    /// [`ConfigField::must_be`](crate::config_field::ConfigField::must_be)'s shape.
    pub(crate) fn check(
        idx: usize,
        expected: ImageDimensions,
        actual: ImageDimensions,
    ) -> Result<(), Self> {
        if actual == expected {
            Ok(())
        } else {
            Err(Self {
                idx,
                expected,
                actual,
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::image::channel_layout::ChannelLayout;
    use crate::image::error::FrameDimensionMismatch;
    use crate::image::image_dimensions::ImageDimensions;

    #[test]
    fn a_frame_mismatch_names_the_frame_and_both_geometries() {
        let expected = ImageDimensions::new((100, 100), ChannelLayout::Rgb);
        assert_eq!(FrameDimensionMismatch::check(2, expected, expected), Ok(()));

        let mismatch = FrameDimensionMismatch::check(
            2,
            expected,
            ImageDimensions::new((200, 100), ChannelLayout::Mono),
        )
        .unwrap_err();
        assert_eq!(
            mismatch.to_string(),
            "frame 2 is 200x100x1, expected 100x100x3"
        );
        // Channels alone are a mismatch: the samples would line up in count but not in meaning.
        assert!(
            FrameDimensionMismatch::check(
                0,
                expected,
                ImageDimensions::new((100, 100), ChannelLayout::Mono)
            )
            .is_err()
        );
    }
}
