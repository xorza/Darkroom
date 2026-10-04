use std::fmt;

use serde::{Deserialize, Serialize};

/// The numeric domain a decoded sample sits in: the span its decoder divided by, where that span
/// came from, the zero point the samples still carry, and the unit.
///
/// Every decode path lands its samples on `[0, 1]`, which makes frames that mean entirely different
/// things look interchangeable. This is what tells them apart, and what converts between them:
/// two frames relate by the affine map [`Self::conversion_to`] gives.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SampleDomain {
    /// Multiply a sample by this to recover the value the source declared, in [`Self::unit`].
    pub scale: f64,
    /// Whether the source declared [`Self::scale`] or the decoder assumed it.
    pub origin: ScaleOrigin,
    /// The zero point the source values still carry, in the source's own units.
    pub pedestal: Pedestal,
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

/// The zero point a source's values carry above true zero signal.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum Pedestal {
    /// No pedestal: a RAW decode subtracts the black level, and a dark subtraction removes the
    /// pedestal the dark carried.
    Removed,
    /// The source values still carry this level, in the source's own units.
    Kept(f64),
    /// The source does not say. Most third-party FITS files keep a camera offset in the data and
    /// record nothing about it.
    Unknown,
}

/// `x ↦ gain·x + offset`: a sample of one [`SampleDomain`] expressed in another.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DomainMap {
    pub gain: f64,
    pub offset: f64,
}

impl DomainMap {
    pub const IDENTITY: Self = Self {
        gain: 1.0,
        offset: 0.0,
    };
}

impl SampleDomain {
    /// The map that expresses a sample of `self` in `target`'s domain, or `None` when the two
    /// cannot be related exactly.
    ///
    /// A sample `x` is `x·scale − pedestal` above zero signal in source units, so in `target` it is
    /// `(x·scale − pedestal + target.pedestal) / target.scale`: the gain is the ratio of the scales
    /// and the offset moves one pedestal onto the other.
    ///
    /// - Different scales relate only when both were declared: an assumed scale is a guess, and
    ///   multiplying by a ratio of guesses would silently rescale a frame that should have been
    ///   refused.
    /// - A known pedestal and an unknown one cannot be related. Two unknown ones are taken as the
    ///   same level, for the reason units are compared only when both are stated: neither frame
    ///   says anything the other contradicts.
    /// - Units are compared only when both frames state one. `BUNIT` is optional, so an absent unit
    ///   is "not stated" rather than "dimensionless". When both do state a unit the comparison is
    ///   exact, case included: `MJy/sr` and `mJy/sr` are a factor of 10⁹ apart and both are in use.
    pub fn conversion_to(&self, target: &Self) -> Option<DomainMap> {
        if !self.units_agree(target) {
            return None;
        }
        let gain = if self.scale == target.scale {
            1.0
        } else if self.origin == ScaleOrigin::Declared && target.origin == ScaleOrigin::Declared {
            self.scale / target.scale
        } else {
            return None;
        };
        let offset = match (self.pedestal.level(), target.pedestal.level()) {
            (Some(from), Some(to)) => (to - from) / target.scale,
            (None, None) => 0.0,
            _ => return None,
        };
        Some(DomainMap { gain, offset })
    }

    /// The pedestal `self` carries after a master of `master`'s domain is subtracted through
    /// [`Self::conversion_to`]. The map puts the master on this frame's pedestal, so the
    /// difference has none, when both are known. An unknown pedestal stays unknown: nothing says
    /// whether the master carried the same one.
    pub const fn after_subtracting(&self, master: &Self) -> Pedestal {
        match (self.pedestal, master.pedestal) {
            (Pedestal::Unknown, _) | (_, Pedestal::Unknown) => Pedestal::Unknown,
            _ => Pedestal::Removed,
        }
    }

    /// Whether the two state no contradicting unit: equal when both state one, and agreeing
    /// whenever either states none — see [`Self::conversion_to`].
    pub fn units_agree(&self, other: &Self) -> bool {
        match (&self.unit, &other.unit) {
            (Some(unit), Some(other)) => unit == other,
            _ => true,
        }
    }
}

impl Pedestal {
    /// The level in source units, `0` when removed, `None` when unknown.
    pub const fn level(self) -> Option<f64> {
        match self {
            Self::Removed => Some(0.0),
            Self::Kept(level) => Some(level),
            Self::Unknown => None,
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
            Some(unit) => write!(f, " {unit}")?,
            None => write!(f, " (no declared unit)")?,
        }
        match self.pedestal {
            Pedestal::Removed => Ok(()),
            Pedestal::Kept(level) => write!(f, ", pedestal {level}"),
            Pedestal::Unknown => write!(f, ", unknown pedestal"),
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::io::image::sample_domain::{DomainMap, Pedestal, SampleDomain, ScaleOrigin};

    fn declared(scale: f64, unit: Option<&str>) -> SampleDomain {
        SampleDomain {
            scale,
            origin: ScaleOrigin::Declared,
            pedestal: Pedestal::Removed,
            unit: unit.map(str::to_owned),
        }
    }

    fn assumed(scale: f64, unit: Option<&str>) -> SampleDomain {
        SampleDomain {
            origin: ScaleOrigin::Assumed,
            ..declared(scale, unit)
        }
    }

    fn with_pedestal(domain: SampleDomain, pedestal: Pedestal) -> SampleDomain {
        SampleDomain { pedestal, ..domain }
    }

    const fn gain(gain: f64) -> DomainMap {
        DomainMap { gain, offset: 0.0 }
    }

    /// The gain is the exact scale ratio between declared domains: 15364 / 15360.
    #[test]
    fn conversion_is_the_exact_scale_ratio_between_declared_domains() {
        for (from, to, expected, reason) in [
            (
                declared(65_535.0, Some("ADU")),
                declared(65_535.0, Some("ADU")),
                Some(gain(1.0)),
                "identical domains",
            ),
            (
                declared(15_364.0, None),
                declared(15_360.0, None),
                Some(gain(15_364.0 / 15_360.0)),
                "two RAW black levels",
            ),
            (
                declared(15_360.0, None),
                declared(15_364.0, None),
                Some(gain(15_360.0 / 15_364.0)),
                "and back",
            ),
            (
                declared(65_535.0, Some("ADU")),
                declared(1.0, Some("ADU")),
                Some(gain(65_535.0)),
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
                Some(gain(1.0)),
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
                Some(gain(1.0)),
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

    /// A dark from a Siril-converted FITS keeps the black level, 2048 ADU, in a
    /// 16-bit domain, and a RAW light has it removed over a span of 15360. The map is gain
    /// 65535/15360 and offset −2048/15360, so a dark sample holding only the pedestal,
    /// 2048/65535, maps to exactly zero in the light's domain instead of subtracting the black a
    /// second time.
    #[test]
    fn a_kept_pedestal_maps_onto_a_removed_one() {
        let dark = with_pedestal(declared(65_535.0, None), Pedestal::Kept(2048.0));
        let light = declared(15_360.0, None);
        let map = dark.conversion_to(&light).unwrap();
        assert_eq!(
            map,
            DomainMap {
                gain: 65_535.0 / 15_360.0,
                offset: -2048.0 / 15_360.0
            }
        );
        assert_eq!(map.gain * (2048.0 / 65_535.0) + map.offset, 0.0);
        assert_eq!(light.after_subtracting(&dark), Pedestal::Removed);

        // Two kept pedestals of the same level on the same scale cancel, and the subtraction takes
        // the light's pedestal with the dark's.
        let kept_light = with_pedestal(declared(65_535.0, None), Pedestal::Kept(2048.0));
        assert_eq!(dark.conversion_to(&kept_light), Some(gain(1.0)));
        assert_eq!(kept_light.after_subtracting(&dark), Pedestal::Removed);
    }

    /// A known pedestal and an unknown one cannot be related; two unknown ones are taken as one
    /// level, and the subtraction then leaves the light's pedestal unknown.
    #[test]
    fn an_unknown_pedestal_relates_only_to_another_unknown_one() {
        let unknown = with_pedestal(declared(65_535.0, None), Pedestal::Unknown);
        let raw = declared(15_360.0, None);
        assert_eq!(unknown.conversion_to(&raw), None);
        assert_eq!(raw.conversion_to(&unknown), None);
        assert_eq!(unknown.conversion_to(&unknown), Some(gain(1.0)));
        assert_eq!(unknown.after_subtracting(&unknown), Pedestal::Unknown);
    }

    #[test]
    fn display_names_the_origin_the_unit_and_the_pedestal() {
        assert_eq!(declared(65_535.0, Some("ADU")).to_string(), "65535 ADU");
        assert_eq!(declared(1.0, None).to_string(), "1 (no declared unit)");
        assert_eq!(
            assumed(1.0, None).to_string(),
            "1 (assumed) (no declared unit)"
        );
        assert_eq!(
            with_pedestal(declared(65_535.0, None), Pedestal::Kept(2048.0)).to_string(),
            "65535 (no declared unit), pedestal 2048"
        );
        assert_eq!(
            with_pedestal(declared(1.0, None), Pedestal::Unknown).to_string(),
            "1 (no declared unit), unknown pedestal"
        );
    }
}
