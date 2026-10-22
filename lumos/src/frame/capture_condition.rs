//! [`CaptureCondition`]: one condition a frame is taken under, and when two values of it count
//! as one.

use std::fmt;

use crate::frame::capture_conditions::CaptureConditions;
use crate::frame::error::ConditionMismatch;

/// Exposures within this share of each other count as one: capture software times frames to a few
/// milliseconds, far inside it, and a dark that matters, such as 120 s on 300 s lights, is far
/// outside.
const EXPOSURE_TOLERANCE: f64 = 0.01;

/// Sensor temperatures within this many degrees count as one: a regulated cooler holds its set
/// point to a few tenths of a degree, and dark current changes by about 12% per degree.
const TEMPERATURE_TOLERANCE: f64 = 1.0;

/// A condition a frame is taken under, which the frames of a dark master have to share.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CaptureCondition {
    /// The exposure time, in seconds.
    Exposure,
    /// The sensor temperature, in degrees Celsius.
    Temperature,
    /// The camera body's temperature, in degrees Celsius.
    CameraTemperature,
}

impl fmt::Display for CaptureCondition {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Exposure => "exposure (s)",
            Self::Temperature => "sensor temperature (°C)",
            Self::CameraTemperature => "camera temperature (°C)",
        })
    }
}

impl CaptureCondition {
    /// This condition's value in `conditions`.
    const fn of(self, conditions: CaptureConditions) -> Option<f64> {
        match self {
            Self::Exposure => conditions.exposure_time,
            Self::Temperature => conditions.ccd_temp,
            Self::CameraTemperature => conditions.camera_temp,
        }
    }

    /// Whether `low ≤ high` count as one value of this condition.
    pub(crate) fn agree(self, low: f64, high: f64) -> bool {
        debug_assert!(low <= high);
        match self {
            Self::Exposure => high - low <= EXPOSURE_TOLERANCE * high,
            Self::Temperature | Self::CameraTemperature => high - low <= TEMPERATURE_TOLERANCE,
        }
    }

    pub(super) fn check_agreement(
        self,
        set: impl IntoIterator<Item = CaptureConditions>,
    ) -> Result<(), ConditionMismatch> {
        let mut range: Option<StatedRange> = None;
        for (idx, conditions) in set.into_iter().enumerate() {
            let Some(value) = self.of(conditions) else {
                continue;
            };
            let Some(range) = &mut range else {
                range = Some(StatedRange::new(idx, value));
                continue;
            };
            let (low, high) = (range.low.min(value), range.high.max(value));
            if !self.agree(low, high) {
                let (reference_idx, reference) = if value > range.high {
                    (range.low_idx, range.low)
                } else {
                    (range.high_idx, range.high)
                };
                return Err(ConditionMismatch {
                    condition: self,
                    idx,
                    value,
                    reference_idx,
                    reference,
                });
            }
            range.include(idx, value);
        }
        Ok(())
    }

    pub(super) fn shared(
        self,
        set: impl IntoIterator<Item = CaptureConditions> + Clone,
    ) -> Option<f64> {
        self.check_agreement(set.clone()).ok()?;
        let mut sum = 0.0;
        let mut count = 0usize;
        for conditions in set {
            sum += self.of(conditions)?;
            count += 1;
        }
        (count > 0).then(|| sum / count as f64)
    }
}

/// The least and the greatest value a set stated so far, with the frames that stated them.
#[derive(Debug)]
struct StatedRange {
    low: f64,
    low_idx: usize,
    high: f64,
    high_idx: usize,
}

impl StatedRange {
    const fn new(idx: usize, value: f64) -> Self {
        Self {
            low: value,
            low_idx: idx,
            high: value,
            high_idx: idx,
        }
    }

    fn include(&mut self, idx: usize, value: f64) {
        if value < self.low {
            self.low = value;
            self.low_idx = idx;
        }
        if value > self.high {
            self.high = value;
            self.high_idx = idx;
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::frame::capture_condition::CaptureCondition;

    #[test]
    fn tolerances_are_inclusive_at_their_bound() {
        // Exposure: 99 and 100 s are exactly 1% of 100 s apart. Temperature: exactly 1 °C.
        assert!(CaptureCondition::Exposure.agree(99.0, 100.0));
        assert!(!CaptureCondition::Exposure.agree(98.9, 100.0));
        assert!(CaptureCondition::Temperature.agree(-11.0, -10.0));
        assert!(!CaptureCondition::Temperature.agree(-11.25, -10.0));
    }
}
