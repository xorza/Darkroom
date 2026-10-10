//! [`CalibrationState`]: the parts of a frame's signal that calibration removed.

/// The parts of a frame's signal that calibration removed from its samples.
///
/// A frame holds the bias and the dark current's signal additively, and the flat field's response
/// multiplicatively. A master that is subtracted removes what it still holds, and no part may be
/// removed twice: a flat-dark stacked with the bias taken from each frame holds the dark signal
/// alone, so the flat it is taken from still needs its bias, and a dark that lost its bias scales
/// with exposure.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct CalibrationState {
    /// The bias: the sensor's offset and its fixed pattern.
    pub bias: bool,
    /// The signal of the dark current.
    pub thermal: bool,
    /// The flat field's response, divided out.
    pub flat: bool,
}

impl CalibrationState {
    /// Nothing removed: a frame as its sensor recorded it.
    pub const NONE: Self = Self {
        bias: false,
        thermal: false,
        flat: false,
    };
    /// The bias alone.
    pub const BIAS: Self = Self {
        bias: true,
        ..Self::NONE
    };
    /// The dark current's signal alone.
    pub const THERMAL: Self = Self {
        thermal: true,
        ..Self::NONE
    };
    /// Both additive parts: what a dark frame records.
    pub const ADDITIVE: Self = Self {
        bias: true,
        thermal: true,
        flat: false,
    };
    /// The flat response alone.
    pub const FLAT: Self = Self {
        flat: true,
        ..Self::NONE
    };

    /// Whether nothing is removed.
    pub const fn is_none(self) -> bool {
        !self.bias && !self.thermal && !self.flat
    }

    /// Both sets' parts.
    #[must_use]
    pub const fn union(self, other: Self) -> Self {
        Self {
            bias: self.bias || other.bias,
            thermal: self.thermal || other.thermal,
            flat: self.flat || other.flat,
        }
    }

    /// The parts of `self` that `other` does not name.
    #[must_use]
    pub const fn without(self, other: Self) -> Self {
        Self {
            bias: self.bias && !other.bias,
            thermal: self.thermal && !other.thermal,
            flat: self.flat && !other.flat,
        }
    }

    /// Whether `self` names every part `other` does.
    pub const fn contains(self, other: Self) -> bool {
        other.without(self).is_none()
    }

    /// Whether `self` and `other` name a part in common.
    pub const fn overlaps(self, other: Self) -> bool {
        (self.bias && other.bias) || (self.thermal && other.thermal) || (self.flat && other.flat)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn set_operations_match_their_parts() {
        let bias_and_flat = CalibrationState::BIAS.union(CalibrationState::FLAT);
        assert_eq!(
            bias_and_flat,
            CalibrationState {
                bias: true,
                thermal: false,
                flat: true
            }
        );
        assert_eq!(
            CalibrationState::ADDITIVE.without(CalibrationState::BIAS),
            CalibrationState::THERMAL
        );
        assert_eq!(
            CalibrationState::ADDITIVE.without(CalibrationState::ADDITIVE),
            CalibrationState::NONE
        );
        assert!(CalibrationState::NONE.is_none());
        assert!(!CalibrationState::THERMAL.is_none());
        assert!(CalibrationState::ADDITIVE.overlaps(CalibrationState::THERMAL));
        assert!(bias_and_flat.overlaps(CalibrationState::ADDITIVE));
        assert!(!CalibrationState::THERMAL.overlaps(bias_and_flat));
        assert!(!CalibrationState::NONE.overlaps(CalibrationState::ADDITIVE));
        assert!(CalibrationState::ADDITIVE.contains(CalibrationState::THERMAL));
        assert!(CalibrationState::ADDITIVE.contains(CalibrationState::NONE));
        assert!(!CalibrationState::BIAS.contains(CalibrationState::ADDITIVE));
        assert!(!bias_and_flat.contains(CalibrationState::THERMAL));
    }
}
