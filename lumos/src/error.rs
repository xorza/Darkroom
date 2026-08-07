//! Errors shared across lumos' modules.

use std::fmt;

/// A configuration field whose value falls outside the range its algorithm accepts.
///
/// Star detection, registration, combining, drizzle and the image ops all reject a field for the
/// same three reasons — which field, what it had to be, what it actually was — so they report it
/// with this one type instead of each growing a variant per field. Constraints that genuinely
/// aren't a range check on a single field (two fields that must agree, a per-element check) keep a
/// typed variant on their module's error.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct InvalidConfigField {
    /// The offending field, spelled as it is in the config struct.
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
    use crate::error::InvalidConfigField;

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
