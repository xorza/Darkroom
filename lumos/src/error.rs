//! Errors shared across lumos' modules.

use std::fmt;

use crate::io::image::image_dimensions::ImageDimensions;

/// A frame whose pixel grid disagrees with the one the run was set up for.
///
/// Loading, alignment, the combine cache and drizzle accumulation all reject a frame for the same
/// three reasons — which frame, the geometry the run expects, what the frame turned out to be — so
/// they report it with this one type instead of each subsystem's error declaring the variant again.
/// The mismatches that genuinely aren't this keep a typed variant on their module's error: a stored
/// plane knows only its length, a pixel-weight map has no channels, and a calibration master is
/// measured against the sensor rather than against frame 0.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FrameDimensionMismatch {
    /// Position of the offending frame in the input set.
    pub index: usize,
    /// The geometry every frame has to match — frame 0's, or the accumulator's.
    pub expected: ImageDimensions,
    /// What this frame turned out to be.
    pub actual: ImageDimensions,
}

impl FrameDimensionMismatch {
    /// `Ok(())` when `actual` matches, else the rejection of frame `index`.
    ///
    /// The Result-returning counterpart to `assert_eq!` for frame geometry, in
    /// [`InvalidConfigField::check`]'s shape — five call sites across loading, alignment, the
    /// combine cache and drizzle each wrote the comparison and the payload out by hand.
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

impl fmt::Display for FrameDimensionMismatch {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "frame {} is {}, expected {}",
            self.index, self.actual, self.expected
        )
    }
}

impl std::error::Error for FrameDimensionMismatch {}

/// A configuration field whose value falls outside the range its algorithm accepts.
///
/// Star detection, registration, combining, drizzle and the image ops all reject a field for the
/// same three reasons — which field, what it had to be, what it actually was — so they report it
/// with this one type instead of each growing a variant per field. Constraints that genuinely
/// aren't a range check on a single field (two fields that must agree, a per-element check) keep a
/// typed variant on their module's error.
#[derive(Debug, Clone, Copy, PartialEq)]
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

impl InvalidConfigField {
    /// `Ok(())` when `accepted` holds, else the rejection of `value` for `field`.
    ///
    /// The Result-returning counterpart to `assert!` for config validation. `expected` completes
    /// the sentence "`<field>` must be …".
    pub(crate) fn check(
        accepted: bool,
        field: &'static str,
        expected: &'static str,
        value: impl Into<f64>,
    ) -> Result<(), Self> {
        if accepted {
            Ok(())
        } else {
            Err(Self {
                field,
                expected,
                value: value.into(),
                bound: None,
            })
        }
    }

    /// [`check`](Self::check) for a float field, whose bound is almost always "finite, and …".
    /// `accepted` states only that second half; finiteness is enforced here so no call site
    /// repeats it.
    pub(crate) fn finite(
        field: &'static str,
        expected: &'static str,
        value: impl Into<f64>,
        accepted: impl FnOnce(f64) -> bool,
    ) -> Result<(), Self> {
        let value = value.into();
        Self::check(value.is_finite() && accepted(value), field, expected, value)
    }

    /// [`finite`](Self::finite) for a field with no bound beyond being a real number.
    pub(crate) fn finite_only(field: &'static str, value: impl Into<f64>) -> Result<(), Self> {
        Self::finite(field, "finite", value, |_| true)
    }

    /// [`check`](Self::check) for a bound that is itself a config value — `expected` names the
    /// other field and `bound` carries what it held, so the message can state both.
    pub(crate) fn check_against(
        accepted: bool,
        field: &'static str,
        expected: &'static str,
        value: impl Into<f64>,
        bound: impl Into<f64>,
    ) -> Result<(), Self> {
        Self::check(accepted, field, expected, value).map_err(|invalid| Self {
            bound: Some(bound.into()),
            ..invalid
        })
    }
}

impl fmt::Display for InvalidConfigField {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} must be {}", self.field, self.expected)?;
        if let Some(bound) = self.bound {
            write!(f, " ({bound})")?;
        }
        write!(f, ", got {}", self.value)
    }
}

impl std::error::Error for InvalidConfigField {}

#[cfg(test)]
mod tests {
    use crate::error::{FrameDimensionMismatch, InvalidConfigField};
    use crate::io::image::image_dimensions::ImageDimensions;

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

    #[test]
    fn a_rejection_states_the_field_the_range_and_the_value() {
        let plain =
            InvalidConfigField::check(false, "sigma_threshold", "finite and positive", 0.0f32)
                .unwrap_err();
        assert_eq!(
            plain.to_string(),
            "sigma_threshold must be finite and positive, got 0"
        );
        assert_eq!(plain.bound, None);

        // A count widens exactly, and prints without a fractional part.
        let tile_size: usize = 300;
        let counted =
            InvalidConfigField::check(false, "tile_size", "between 16 and 256", tile_size as f64)
                .unwrap_err();
        assert_eq!(
            counted.to_string(),
            "tile_size must be between 16 and 256, got 300"
        );

        let against =
            InvalidConfigField::check_against(false, "max_area", "at least min_area", 3u32, 5u32)
                .unwrap_err();
        assert_eq!(
            against.to_string(),
            "max_area must be at least min_area (5), got 3"
        );
        assert_eq!(against.bound, Some(5.0));

        assert_eq!(
            InvalidConfigField::check(true, "sigma_threshold", "finite and positive", f32::NAN),
            Ok(())
        );
    }
}
