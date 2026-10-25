//! [`DarkComparison`]: what taking a master of dark signal from a frame compared, and what it
//! found.

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::image::metadata::temperature_difference::TemperatureDifference;
use crate::image::metadata::temperature_source::TemperatureSource;
use crate::image::metadata::unverified_conditions::UnverifiedConditions;

/// What a master holding dark signal was compared to the frame it was taken from on, and what the
/// comparison found: a light's dark, a flat's flat-dark. The one record a calibrated frame carries,
/// a light's [`CalibrationOutcome`](crate::CalibrationOutcome) reports, and a master stacked from
/// such frames keeps as the [union](Self::union) of its frames'.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct DarkComparison {
    /// Whether the exposures were compared: both stated one.
    pub exposure_compared: bool,
    /// The reading the temperatures were compared on: the sensor's where both state it, else the
    /// camera body's; `None` when they share neither.
    pub temperature: Option<TemperatureSource>,
    /// The two temperatures, when they differ past the tolerance: a warning, not a refusal.
    pub temperature_difference: Option<TemperatureDifference>,
}

impl DarkComparison {
    /// The conditions not compared.
    pub const fn unverified(&self) -> UnverifiedConditions {
        UnverifiedConditions {
            exposure: !self.exposure_compared,
            temperature: self.temperature.is_none(),
        }
    }

    /// The comparison of a master stacked from a frame compared as `self` and one compared as
    /// `other`: what either left uncompared, the weaker reading the temperatures were compared on,
    /// and the larger temperature difference.
    #[must_use]
    pub fn union(self, other: Self) -> Self {
        let strength = |reading: Option<TemperatureSource>| match reading {
            None => 0,
            Some(TemperatureSource::Camera) => 1,
            Some(TemperatureSource::Sensor) => 2,
        };
        let spread =
            |difference: &TemperatureDifference| (difference.frame - difference.dark).abs();
        Self {
            exposure_compared: self.exposure_compared && other.exposure_compared,
            temperature: if strength(other.temperature) < strength(self.temperature) {
                other.temperature
            } else {
                self.temperature
            },
            temperature_difference: match (
                self.temperature_difference,
                other.temperature_difference,
            ) {
                (Some(own), Some(theirs)) if spread(&theirs) > spread(&own) => Some(theirs),
                (own, theirs) => own.or(theirs),
            },
        }
    }
}

impl fmt::Display for DarkComparison {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.temperature {
            Some(reading) => write!(f, "temperature compared on the {reading} reading")?,
            None => f.write_str("no temperature compared")?,
        }
        if !self.exposure_compared {
            f.write_str(", exposure not compared")?;
        }
        if let Some(difference) = self.temperature_difference {
            write!(f, "; {difference}")?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use crate::image::metadata::dark_comparison::DarkComparison;
    use crate::image::metadata::temperature_difference::TemperatureDifference;
    use crate::image::metadata::temperature_source::TemperatureSource;
    use crate::image::metadata::unverified_conditions::UnverifiedConditions;

    /// Two frames of a stack: one compared on the sensor at 2 °C apart, its exposure stated; one on
    /// the camera body at 3 °C apart, its exposure not. The stack's record is the weaker reading
    /// (the camera's), the larger difference (3 °C, the second's), and the exposure the second left
    /// uncompared; in either order, the union being symmetric.
    #[test]
    fn a_union_keeps_the_weaker_reading_and_the_larger_difference() {
        let sensor = DarkComparison {
            exposure_compared: true,
            temperature: Some(TemperatureSource::Sensor),
            temperature_difference: Some(TemperatureDifference {
                reading: TemperatureSource::Sensor,
                frame: -10.0,
                dark: -12.0,
            }),
        };
        let camera_difference = TemperatureDifference {
            reading: TemperatureSource::Camera,
            frame: 20.0,
            dark: 23.0,
        };
        let camera = DarkComparison {
            exposure_compared: false,
            temperature: Some(TemperatureSource::Camera),
            temperature_difference: Some(camera_difference),
        };
        let expected = DarkComparison {
            exposure_compared: false,
            temperature: Some(TemperatureSource::Camera),
            temperature_difference: Some(camera_difference),
        };
        assert_eq!(sensor.union(camera), expected);
        assert_eq!(camera.union(sensor), expected);
        assert_eq!(
            expected.unverified(),
            UnverifiedConditions {
                exposure: true,
                temperature: false,
            }
        );
        let unshared = DarkComparison {
            exposure_compared: true,
            temperature: None,
            temperature_difference: None,
        };
        assert_eq!(sensor.union(unshared).temperature, None);
        assert_eq!(
            sensor.union(unshared).temperature_difference,
            sensor.temperature_difference
        );
        assert!(sensor.union(unshared).unverified().temperature);
    }
}
