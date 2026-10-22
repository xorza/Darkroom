//! [`ConfigField`]: a configuration field's value, checked against the range its algorithm
//! accepts.

use crate::error::InvalidConfigField;

/// A configuration field's value, with the name an [`InvalidConfigField`] reports it by.
///
/// Each check is the Result-returning counterpart to `assert!` for config validation: `expected`
/// completes the sentence "`<field>` must be …".
#[derive(Debug, Clone, Copy)]
pub(crate) struct ConfigField {
    field: &'static str,
    value: f64,
}

impl ConfigField {
    /// `value` of `field`, named as [`InvalidConfigField::field`] says.
    pub(crate) fn new(field: &'static str, value: impl Into<f64>) -> Self {
        Self {
            field,
            value: value.into(),
        }
    }

    /// [`Self::new`] for a count, which `usize` has no lossless conversion to `f64` for — see
    /// [`exact`].
    pub(crate) fn count(field: &'static str, count: usize) -> Self {
        Self {
            field,
            value: exact(count),
        }
    }

    /// `Ok(())` when `accepted` holds, else the rejection of the value.
    pub(crate) const fn must_be(
        self,
        expected: &'static str,
        accepted: bool,
    ) -> Result<(), InvalidConfigField> {
        if accepted {
            Ok(())
        } else {
            Err(InvalidConfigField {
                field: self.field,
                expected,
                value: self.value,
                bound: None,
            })
        }
    }

    /// [`Self::must_be`] for a float field, whose bound is almost always "finite, and …".
    /// `accepted` states only that second half; finiteness is enforced here so no call site
    /// repeats it.
    pub(crate) fn finite_and(
        self,
        expected: &'static str,
        accepted: impl FnOnce(f64) -> bool,
    ) -> Result<(), InvalidConfigField> {
        self.must_be(expected, self.value.is_finite() && accepted(self.value))
    }

    /// [`Self::finite_and`] for a field with no bound beyond being a real number.
    pub(crate) fn finite(self) -> Result<(), InvalidConfigField> {
        self.finite_and("finite", |_| true)
    }

    /// [`Self::must_be`] for a bound that is itself a config value — `expected` names the other
    /// field and `bound` carries what it held, so the message can state both.
    pub(crate) fn must_be_against(
        self,
        expected: &'static str,
        bound: impl Into<f64>,
        accepted: bool,
    ) -> Result<(), InvalidConfigField> {
        self.must_be(expected, accepted)
            .map_err(|invalid| InvalidConfigField {
                bound: Some(bound.into()),
                ..invalid
            })
    }

    /// [`Self::must_be_against`] for a bound that is a count.
    pub(crate) fn must_be_against_count(
        self,
        expected: &'static str,
        bound: usize,
        accepted: bool,
    ) -> Result<(), InvalidConfigField> {
        self.must_be_against(expected, exact(bound), accepted)
    }
}

/// A config count as `f64`. A count a configuration states stays far below 2^53, past which `f64`
/// skips integers, so the conversion is exact.
fn exact(count: usize) -> f64 {
    debug_assert!(count <= 1 << 53, "a config count is exact in f64: {count}");
    count as f64
}

#[cfg(test)]
mod tests {
    use crate::config_field::ConfigField;

    #[test]
    fn a_rejection_states_the_field_the_range_and_the_value() {
        let plain = ConfigField::new("sigma_threshold", 0.0f32)
            .must_be("finite and positive", false)
            .unwrap_err();
        assert_eq!(
            plain.to_string(),
            "sigma_threshold must be finite and positive, got 0"
        );
        assert_eq!(plain.bound, None);

        // A count widens exactly, and prints without a fractional part.
        let counted = ConfigField::count("tile_size", 300)
            .must_be("between 16 and 256", false)
            .unwrap_err();
        assert_eq!(
            counted.to_string(),
            "tile_size must be between 16 and 256, got 300"
        );

        let against = ConfigField::new("max_area", 3u32)
            .must_be_against("at least min_area", 5u32, false)
            .unwrap_err();
        assert_eq!(
            against.to_string(),
            "max_area must be at least min_area (5), got 3"
        );
        assert_eq!(against.bound, Some(5.0));

        // `must_be` takes the caller's verdict alone; `finite_and` adds finiteness to it.
        let nan = ConfigField::new("sigma_threshold", f32::NAN);
        assert_eq!(nan.must_be("finite and positive", true), Ok(()));
        let rejected = nan.finite_and("finite and positive", |_| true).unwrap_err();
        assert_eq!(
            (rejected.field, rejected.expected, rejected.bound),
            ("sigma_threshold", "finite and positive", None)
        );
        assert!(rejected.value.is_nan());
        assert_eq!(
            ConfigField::new("alpha", 0.5).finite_and("finite and in (0, 1)", |v| v < 1.0),
            Ok(())
        );
        assert!(
            ConfigField::new("alpha", 1.5)
                .finite_and("finite and in (0, 1)", |v| v < 1.0)
                .is_err()
        );
        assert_eq!(ConfigField::new("gain", -3.0).finite(), Ok(()));
        assert_eq!(
            ConfigField::new("gain", f64::INFINITY)
                .finite()
                .unwrap_err()
                .to_string(),
            "gain must be finite, got inf"
        );
    }
}
