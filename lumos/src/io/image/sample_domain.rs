use std::fmt;

use serde::{Deserialize, Serialize};

/// The numeric domain a decoded sample sits in: the span its decoder divided by, and the unit that
/// span was expressed in.
///
/// Every decode path lands its samples on `[0, 1]`, which makes frames that mean entirely different
/// things look interchangeable. This is what tells them apart — two frames may be combined only
/// when [`Self::commensurate_with`] holds.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SampleDomain {
    /// Multiply a sample by this to recover the value the source declared, in [`Self::unit`].
    pub scale: f32,
    /// The unit that recovered value is in — FITS `BUNIT` — or `None` when the source declares
    /// none.
    ///
    /// Sensor formats state no unit at all; FITS frequently does, and `ADU`, `electron`, `count/s`
    /// and `Jy/beam` are all in circulation for data that is otherwise shaped identically.
    pub unit: Option<String>,
}

impl SampleDomain {
    /// Whether `other`'s samples mean the same thing as these, so the two can be combined without a
    /// conversion between them.
    ///
    /// The scales must match exactly: a ratio between them is a systematic gain error, and
    /// `Normalization::Global` would absorb it into its fitted gain and return a plausible-looking
    /// result.
    ///
    /// Units are compared only when both frames state one. `BUNIT` is optional, so an absent unit
    /// is "not stated" rather than "dimensionless" — treating it as a value that disagrees with
    /// every stated one would reject a RAW light against a FITS master over metadata neither frame
    /// contradicts. This makes the relation non-transitive across frames that state no unit, which
    /// is why [`validate_sample_domains`](crate::stacking::combine::cache::validation::validate_sample_domains)
    /// tracks the two attributes against separate references rather than folding a whole set
    /// through this.
    ///
    /// When both do state a unit the comparison is exact, case included: `MJy/sr` and `mJy/sr` are
    /// a factor of 10⁹ apart and both are in use, so case-folding would conflate exactly the
    /// mismatch this exists to catch.
    pub fn commensurate_with(&self, other: &Self) -> bool {
        self.scale == other.scale
            && match (&self.unit, &other.unit) {
                (Some(unit), Some(other)) => unit == other,
                _ => true,
            }
    }
}

impl fmt::Display for SampleDomain {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.unit {
            Some(unit) => write!(f, "{} {unit}", self.scale),
            None => write!(f, "{} (no declared unit)", self.scale),
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::io::image::sample_domain::SampleDomain;

    fn domain(scale: f32, unit: Option<&str>) -> SampleDomain {
        SampleDomain {
            scale,
            unit: unit.map(str::to_owned),
        }
    }

    #[test]
    fn commensurability_needs_an_equal_scale_and_no_stated_disagreement_on_unit() {
        for (left, right, expected, reason) in [
            (
                domain(65_535.0, Some("ADU")),
                domain(65_535.0, Some("ADU")),
                true,
                "identical domains",
            ),
            (
                domain(65_535.0, Some("ADU")),
                domain(1.0, Some("ADU")),
                false,
                "a 65535x span difference",
            ),
            (
                domain(1.0, Some("Jy/beam")),
                domain(1.0, Some("count/s")),
                false,
                "the same span in two different units",
            ),
            (
                domain(1.0, Some("MJy/sr")),
                domain(1.0, Some("mJy/sr")),
                false,
                "mega- against milli-, a factor of 1e9",
            ),
            (
                domain(1.0, Some("ADU")),
                domain(1.0, None),
                true,
                "one side states no unit, which is not a disagreement",
            ),
            (
                domain(1.0, None),
                domain(1.0, None),
                true,
                "neither side states a unit",
            ),
            (
                domain(1.0, Some("ADU")),
                domain(2.0, None),
                false,
                "an unstated unit does not excuse a differing scale",
            ),
        ] {
            assert_eq!(
                left.commensurate_with(&right),
                expected,
                "{reason}: {left} vs {right}"
            );
            assert_eq!(
                right.commensurate_with(&left),
                expected,
                "{reason}, reversed: {right} vs {left}"
            );
        }
    }

    #[test]
    fn display_names_the_unit_when_there_is_one() {
        assert_eq!(domain(65_535.0, Some("ADU")).to_string(), "65535 ADU");
        assert_eq!(domain(1.0, None).to_string(), "1 (no declared unit)");
    }
}
