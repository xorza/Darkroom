//! [`TemperatureSource`]: which temperature a dark was matched to a frame on.

use std::fmt;

/// Which temperature a dark was matched to a frame on: the sensor's, where both state it, else the
/// camera body's, which some makers record instead — Canon, Kodak, Leica, Pentax, Samsung.
///
/// A body reads warmer than its sensor and lags it, so a match on it is the weaker check; the
/// outcome says which one was made.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TemperatureSource {
    /// The sensor's temperature: a cooled camera's, or a maker note's sensor reading.
    Sensor,
    /// The camera body's temperature from its maker notes.
    Camera,
}

impl fmt::Display for TemperatureSource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Sensor => "sensor",
            Self::Camera => "camera",
        })
    }
}
