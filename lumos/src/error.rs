//! Errors shared across lumos' modules.

use common::CancelToken;
use thiserror::Error;

use crate::image::image_dimensions::ImageDimensions;

/// A frame whose pixel grid disagrees with the one the run was set up for.
///
/// Loading, alignment, the combine cache and drizzle accumulation all reject a frame for the same
/// three reasons — which frame, the geometry the run expects, what the frame turned out to be — so
/// they report it with this one type instead of each subsystem's error declaring the variant again.
/// The mismatches that genuinely aren't this keep a typed variant on their module's error: a stored
/// plane knows only its length, a pixel-weight map has no channels, and a calibration master is
/// measured against the sensor rather than against frame 0.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
#[error("frame {index} is {actual}, expected {expected}")]
pub struct FrameDimensionMismatch {
    /// Position of the offending frame in the input set.
    pub index: usize,
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
            index: inputs[self.index],
            ..self
        }
    }

    /// `Ok(())` when `actual` matches, else the rejection of frame `index`: the Result-returning
    /// counterpart to `assert_eq!` for frame geometry, in
    /// [`ConfigValue::must_be`](crate::config_value::ConfigValue::must_be)'s shape.
    pub(crate) fn check(
        index: usize,
        expected: ImageDimensions,
        actual: ImageDimensions,
    ) -> Result<(), Self> {
        if actual == expected {
            Ok(())
        } else {
            Err(Self {
                index,
                expected,
                actual,
            })
        }
    }
}

/// A configuration field whose value falls outside the range its algorithm accepts.
///
/// Star detection, registration, combining, drizzle and the image ops all reject a field for the
/// same three reasons — which field, what it had to be, what it actually was — so they report it
/// with this one type instead of each growing a variant per field. Constraints that genuinely
/// aren't a range check on a single field (two fields that must agree, a per-element check) keep a
/// typed variant on their module's error.
#[derive(Debug, Clone, Copy, PartialEq, Error)]
#[error(
    "{field} must be {expected}{}, got {value}",
    .bound.map_or_else(String::new, |bound| format!(" ({bound})"))
)]
pub struct InvalidConfigField {
    /// The offending field. Its name in the config struct, prefixed with the algorithm that owns
    /// it when the bare name would not identify it — `tile_size` alone names two unrelated fields.
    pub field: &'static str,
    /// The accepted range as prose, completing "`<field>` must be …": `"finite and positive"`,
    /// `"between 16 and 256"`.
    pub expected: &'static str,
    /// The rejected value. Integer fields widen into it exactly — config counts stay far below
    /// f64's 2^53 integer limit.
    pub value: f64,
    /// The other operand when `expected` names a second field rather than a constant bound.
    pub bound: Option<f64>,
}

/// Returned by a decode or demosaic stage that saw its cancel token set. A marker only: the
/// partial buffers are dropped, and the caller reports the cancellation as its own error.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Cancelled;

impl Cancelled {
    /// How many samples a cancellable walk covers between cancel polls.
    ///
    /// The frame checks and the normalization gather chunk their iteration to this rather than
    /// test the token per sample. The figure is not tuned to either: it is a granularity, small
    /// enough that a cancelled run stops promptly and large enough that the poll is lost in the
    /// work.
    pub(crate) const POLL_CHUNK: usize = 16_384;

    /// `Err(Cancelled)` once `cancel` is set, so a long walk can `?` its way out between chunks.
    pub(crate) fn check(cancel: &CancelToken) -> Result<(), Self> {
        if cancel.is_cancelled() {
            Err(Self)
        } else {
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::error::FrameDimensionMismatch;
    use crate::image::image_dimensions::ImageDimensions;

    #[test]
    fn a_frame_mismatch_names_the_frame_and_both_geometries() {
        let expected = ImageDimensions::new((100, 100), 3);
        assert_eq!(FrameDimensionMismatch::check(2, expected, expected), Ok(()));

        let mismatch =
            FrameDimensionMismatch::check(2, expected, ImageDimensions::new((200, 100), 1))
                .unwrap_err();
        assert_eq!(
            mismatch.to_string(),
            "frame 2 is 200x100x1, expected 100x100x3"
        );
        // Channels alone are a mismatch: the samples would line up in count but not in meaning.
        assert!(
            FrameDimensionMismatch::check(0, expected, ImageDimensions::new((100, 100), 1))
                .is_err()
        );
    }
}
