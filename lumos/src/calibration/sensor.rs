//! [`Sensor`]: the pattern and extent a frame, a master or a defect map is of.

use crate::calibration::calibration_component::CalibrationComponent;
use crate::calibration::defect_map::DefectMap;
use crate::calibration::error::CalibrationError;
use crate::image::cfa::cfa_image::CfaImage;
use crate::image::cfa::cfa_type::CfaType;
use common::Size2us;

/// The sensor a component is of: its CFA pattern, which a defect map does not state, and its
/// extent.
///
/// Masters are combined with each other and with a light pixel for pixel by flat index — a dark
/// subtracted from a flat, defects detected on one and corrected on another — so every component
/// that meets another has to be of one sensor. Masters are read from user-chosen files, so one of
/// another sensor is bad input to refuse, naming its role, not an invariant to assert.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Sensor {
    cfa_type: Option<CfaType>,
    size: Size2us,
}

impl Sensor {
    pub(crate) const fn of(image: &CfaImage) -> Self {
        Self {
            cfa_type: Some(image.cfa_type()),
            size: image.size(),
        }
    }

    pub(super) const fn of_defects(map: &DefectMap) -> Self {
        Self {
            cfa_type: None,
            size: map.size(),
        }
    }

    pub(crate) const fn size(self) -> Size2us {
        self.size
    }

    /// The sensor every one of `components` is of, or `None` when there are none: the first's,
    /// each of the rest refused as [`Self::admit`] refuses it. A defect map states no pattern, so
    /// it goes last.
    pub(crate) fn shared(
        components: impl IntoIterator<Item = (CalibrationComponent, Self)>,
    ) -> Result<Option<Self>, CalibrationError> {
        let mut components = components.into_iter();
        let Some((_, first)) = components.next() else {
            return Ok(None);
        };
        first.admit(components)?;
        Ok(Some(first))
    }

    /// Refuse the first of `components` that is not of this sensor: of another pattern, where it
    /// states one, then of another extent.
    pub(crate) fn admit(
        self,
        components: impl IntoIterator<Item = (CalibrationComponent, Self)>,
    ) -> Result<(), CalibrationError> {
        for (component, sensor) in components {
            if let (Some(expected), Some(master)) = (self.cfa_type, sensor.cfa_type)
                && master != expected
            {
                let CalibrationComponent::Master(component) = component else {
                    unreachable!("a defect map states no pattern")
                };
                return Err(CalibrationError::CfaPatternMismatch {
                    component,
                    expected,
                    master,
                });
            }
            if sensor.size != self.size {
                return Err(CalibrationError::DimensionMismatch {
                    component,
                    expected: self.size,
                    master: sensor.size,
                });
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use crate::calibration::calibration_component::CalibrationComponent;
    use crate::calibration::defect_map::DefectMap;
    use crate::calibration::error::CalibrationError;
    use crate::calibration::master_role::MasterRole;
    use crate::calibration::sensor::Sensor;
    use crate::image::cfa::bayer_pattern::BayerPattern;
    use crate::image::cfa::cfa_type::CfaType;
    use crate::image::cfa::mosaic::Mosaic;
    use crate::internals::cfa::constant_cfa;
    use common::Size2us;

    /// The first component sets the sensor, and each of the rest is refused on its pattern before
    /// its extent: a dark of another pattern and size reads as the pattern. A defect map states
    /// no pattern, so only its extent is compared. None at all share no sensor.
    #[test]
    fn the_first_component_sets_the_sensor_the_rest_are_refused_against() {
        let rggb = CfaType::Mosaic(Mosaic::Bayer(BayerPattern::Rggb));
        let bias = constant_cfa(Size2us::new(2, 2), 0.1, rggb);
        let dark = constant_cfa(Size2us::new(4, 2), 0.1, CfaType::Mono);
        let master =
            |role: MasterRole, image| (CalibrationComponent::Master(role), Sensor::of(image));
        assert_eq!(
            Sensor::shared([
                master(MasterRole::Bias, &bias),
                master(MasterRole::Dark, &dark)
            ]),
            Err(CalibrationError::CfaPatternMismatch {
                component: MasterRole::Dark,
                expected: rggb,
                master: CfaType::Mono,
            })
        );
        let defects = DefectMap::new(Size2us::new(2, 4));
        assert_eq!(
            Sensor::shared([
                master(MasterRole::Bias, &bias),
                (CalibrationComponent::Defects, Sensor::of_defects(&defects)),
            ]),
            Err(CalibrationError::DimensionMismatch {
                component: CalibrationComponent::Defects,
                expected: Size2us::new(2, 2),
                master: Size2us::new(2, 4),
            })
        );
        let same = DefectMap::new(Size2us::new(2, 2));
        assert_eq!(
            Sensor::shared([
                master(MasterRole::Bias, &bias),
                (CalibrationComponent::Defects, Sensor::of_defects(&same)),
            ]),
            Ok(Some(Sensor::of(&bias)))
        );
        assert_eq!(Sensor::shared([]), Ok(None));
    }
}
