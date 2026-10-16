//! [`DarkComparisonKeywords`]: a [`DarkComparison`] as header keywords.

use fits_well::header::Header;

use crate::image::dark_comparison::DarkComparison;
use crate::image::temperature_difference::TemperatureDifference;
use crate::image::temperature_source::TemperatureSource;
use crate::io::fits::metadata::optional;

/// The keywords one comparison is written under: an image's own, or the flat-dark's a stack
/// product's report states.
#[derive(Debug, Clone, Copy)]
pub(crate) struct DarkComparisonKeywords {
    /// The reading the temperatures were compared on, [`SENSOR`], [`CAMERA`] or [`NONE`]: present
    /// exactly when a comparison is recorded.
    reading: &'static str,
    /// Whether the exposures were compared.
    exposure: &'static str,
    /// The reading the temperatures differed on, then the frame's and the dark's, in °C: present
    /// when the two differ past the tolerance.
    difference: [&'static str; 3],
}

const SENSOR: &str = "SENSOR";
const CAMERA: &str = "CAMERA";
const NONE: &str = "NONE";

impl DarkComparisonKeywords {
    /// The keywords of the comparison an image's own record states.
    pub(crate) const IMAGE: Self = Self {
        reading: "LUMDKRD",
        exposure: "LUMDKEXP",
        difference: ["LUMDKDRD", "LUMDKDFR", "LUMDKDDK"],
    };

    /// The keywords of the flat-dark's comparison, which a stack product's report states.
    pub(crate) const FLAT_DARK: Self = Self {
        reading: "LUMFDRD",
        exposure: "LUMFDEXP",
        difference: ["LUMFDDRD", "LUMFDDFR", "LUMFDDDK"],
    };

    /// Write `comparison` into `header`; nothing for none.
    pub(crate) fn write(
        self,
        header: &mut Header,
        comparison: Option<DarkComparison>,
    ) -> fits_well::Result<()> {
        let Some(comparison) = comparison else {
            return Ok(());
        };
        header.set(self.reading, reading_keyword(comparison.temperature))?;
        header.set(self.exposure, comparison.exposure_compared)?;
        if let Some(difference) = comparison.temperature_difference {
            let [reading, frame, dark] = self.difference;
            header.set(reading, reading_keyword(Some(difference.reading)))?;
            header.set(frame, difference.frame)?;
            header.set(dark, difference.dark)?;
        }
        Ok(())
    }

    /// The comparison `header` states; `None` when it states none. A value given in a form it
    /// cannot have is logged and read as absent, as every optional keyword is.
    pub(crate) fn read(self, header: &Header) -> Option<DarkComparison> {
        let reading = optional(self.reading, header.get_text(self.reading))?;
        let temperature = if reading == NONE {
            None
        } else {
            Some(source_of(reading)?)
        };
        let exposure_compared = optional(self.exposure, header.get_logical(self.exposure))?;
        let [reading, frame, dark] = self.difference;
        let temperature_difference = optional(reading, header.get_text(reading))
            .and_then(source_of)
            .zip(optional(frame, header.get_real(frame)))
            .zip(optional(dark, header.get_real(dark)))
            .map(|((reading, frame), dark)| TemperatureDifference {
                reading,
                frame,
                dark,
            });
        Some(DarkComparison {
            exposure_compared,
            temperature,
            temperature_difference,
        })
    }
}

const fn reading_keyword(reading: Option<TemperatureSource>) -> &'static str {
    match reading {
        Some(TemperatureSource::Sensor) => SENSOR,
        Some(TemperatureSource::Camera) => CAMERA,
        None => NONE,
    }
}

/// The reading `value` names; `None`, logged, for a value that names none.
fn source_of(value: &str) -> Option<TemperatureSource> {
    match value {
        SENSOR => Some(TemperatureSource::Sensor),
        CAMERA => Some(TemperatureSource::Camera),
        value => {
            tracing::warn!(value, "ignoring a temperature reading no comparison names");
            None
        }
    }
}
