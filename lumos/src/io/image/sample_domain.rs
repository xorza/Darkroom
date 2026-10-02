use std::fmt;

use serde::{Deserialize, Serialize};

/// The numeric domain a decoded sample sits in: the span its decoder divided by, where that span
/// came from, and the unit it was expressed in.
///
/// Every decode path lands its samples on `[0, 1]`, which makes frames that mean entirely different
/// things look interchangeable. This is what tells them apart, and what converts between them:
/// two frames with declared scales in the same unit relate by the exact ratio of their scales
/// ([`Self::conversion_to`]).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SampleDomain {
    /// Multiply a sample by this to recover the value the source declared, in [`Self::unit`].
    pub scale: f32,
    /// Whether the source declared [`Self::scale`] or the decoder assumed it.
    pub origin: ScaleOrigin,
    /// The unit that recovered value is in — FITS `BUNIT` — or `None` when the source declares
    /// none.
    ///
    /// Sensor formats state no unit at all; FITS frequently does, and `ADU`, `electron`, `count/s`
    /// and `Jy/beam` are all in circulation for data that is otherwise shaped identically.
    pub unit: Option<String>,
}

/// Where a [`SampleDomain::scale`] came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ScaleOrigin {
    /// The source states what one decoded unit is worth in its own physical terms: a RAW file's
    /// `maximum − black`, an integer FITS `BITPIX` range, a floating-point full scale the caller
    /// supplied, or the scale a lumos-written FITS recorded.
    Declared,
    /// The decoder chose it for lack of such a statement: a floating-point FITS read under
    /// `FitsFloatScale::Auto` or taken as already normalized, or a floating-point raster. Two
    /// such scales agree only when equal.
    Assumed,
}

impl SampleDomain {
    /// The factor that expresses a sample of `self` in `target`'s domain, or `None` when the two
    /// cannot be related exactly.
    ///
    /// Equal scales relate by `1.0`. Different scales relate by `self.scale / target.scale` only
    /// when both were declared: an assumed scale is a guess, and multiplying by a ratio of guesses
    /// would silently rescale a frame that should have been refused.
    ///
    /// Units are compared only when both frames state one. `BUNIT` is optional, so an absent unit
    /// is "not stated" rather than "dimensionless"; treating it as a value that disagrees with
    /// every stated one would reject a RAW light against a FITS master over metadata neither frame
    /// contradicts. When both do state a unit the comparison is exact, case included: `MJy/sr` and
    /// `mJy/sr` are a factor of 10⁹ apart and both are in use.
    pub fn conversion_to(&self, target: &Self) -> Option<f32> {
        if !self.units_agree(target) {
            return None;
        }
        if self.scale == target.scale {
            return Some(1.0);
        }
        (self.origin == ScaleOrigin::Declared && target.origin == ScaleOrigin::Declared)
            .then(|| (f64::from(self.scale) / f64::from(target.scale)) as f32)
    }
}

impl SampleDomain {
    /// Whether the two state no contradicting unit: equal when both state one, and agreeing
    /// whenever either states none — see [`Self::conversion_to`].
    pub fn units_agree(&self, other: &Self) -> bool {
        match (&self.unit, &other.unit) {
            (Some(unit), Some(other)) => unit == other,
            _ => true,
        }
    }
}

impl fmt::Display for SampleDomain {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.scale)?;
        if self.origin == ScaleOrigin::Assumed {
            write!(f, " (assumed)")?;
        }
        match &self.unit {
            Some(unit) => write!(f, " {unit}"),
            None => write!(f, " (no declared unit)"),
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::io::image::sample_domain::{SampleDomain, ScaleOrigin};

    fn declared(scale: f32, unit: Option<&str>) -> SampleDomain {
        SampleDomain {
            scale,
            origin: ScaleOrigin::Declared,
            unit: unit.map(str::to_owned),
        }
    }

    fn assumed(scale: f32, unit: Option<&str>) -> SampleDomain {
        SampleDomain {
            origin: ScaleOrigin::Assumed,
            ..declared(scale, unit)
        }
    }

    /// Hand-computed: 15364 / 15360 in f64 is 1.000260416…, which rounds to the f32 1.0002604.
    #[test]
    fn conversion_is_the_exact_scale_ratio_between_declared_domains() {
        for (from, to, expected, reason) in [
            (
                declared(65_535.0, Some("ADU")),
                declared(65_535.0, Some("ADU")),
                Some(1.0),
                "identical domains",
            ),
            (
                declared(15_364.0, None),
                declared(15_360.0, None),
                Some((15_364.0f64 / 15_360.0) as f32),
                "two RAW black levels",
            ),
            (
                declared(15_360.0, None),
                declared(15_364.0, None),
                Some((15_360.0f64 / 15_364.0) as f32),
                "and back",
            ),
            (
                declared(65_535.0, Some("ADU")),
                declared(1.0, Some("ADU")),
                Some(65_535.0),
                "16-bit ADU to unit scale",
            ),
            (
                declared(1.0, Some("Jy/beam")),
                declared(1.0, Some("count/s")),
                None,
                "the same span in two units",
            ),
            (
                declared(1.0, Some("MJy/sr")),
                declared(1.0, Some("mJy/sr")),
                None,
                "mega- against milli-",
            ),
            (
                declared(1.0, Some("ADU")),
                declared(1.0, None),
                Some(1.0),
                "one side states no unit",
            ),
            (
                assumed(1.0, None),
                declared(15_360.0, None),
                None,
                "an assumed scale is not converted",
            ),
            (
                declared(15_360.0, None),
                assumed(1.0, None),
                None,
                "in either direction",
            ),
            (
                assumed(1.0, None),
                assumed(1.0, None),
                Some(1.0),
                "equal assumed scales agree",
            ),
            (
                assumed(1.0, None),
                assumed(65_535.0, None),
                None,
                "two different guesses",
            ),
        ] {
            assert_eq!(
                from.conversion_to(&to),
                expected,
                "{reason}: {from} -> {to}"
            );
        }
    }

    #[test]
    fn display_names_the_origin_and_the_unit() {
        assert_eq!(declared(65_535.0, Some("ADU")).to_string(), "65535 ADU");
        assert_eq!(declared(1.0, None).to_string(), "1 (no declared unit)");
        assert_eq!(
            assumed(1.0, None).to_string(),
            "1 (assumed) (no declared unit)"
        );
    }
}
