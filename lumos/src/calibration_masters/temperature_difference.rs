//! [`TemperatureDifference`]: a dark taken at another temperature than the frame it calibrates.

use std::fmt;

use crate::calibration_masters::temperature_source::TemperatureSource;

/// A master holding dark signal taken at another temperature than the frame it was subtracted
/// from, past the 1 °C tolerance, by the reading both state. A warning, not a refusal, as no
/// reference refuses on temperature: dark current changes by about 12% per degree, so a dark not
/// scaled for the difference removes the wrong amount of it. A scale fitted on the hot pixels
/// corrects that.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TemperatureDifference {
    pub reading: TemperatureSource,
    /// The frame's temperature, °C.
    pub frame: f64,
    /// The dark's temperature, °C.
    pub dark: f64,
}

impl fmt::Display for TemperatureDifference {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "the dark's {} temperature {} °C differs from the frame's {} °C",
            self.reading, self.dark, self.frame
        )
    }
}
