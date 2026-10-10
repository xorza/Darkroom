//! [`CaptureConditions`]: the exposure and sensor temperature a frame was taken under.

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::frame_store::error::ConditionMismatch;
use crate::io::image::image_metadata::ImageMetadata;

/// Exposures within this share of each other count as one: capture software times frames to a few
/// milliseconds, far inside it, and a dark that matters, such as 120 s on 300 s lights, is far
/// outside.
pub(crate) const EXPOSURE_TOLERANCE: f64 = 0.01;

/// Sensor temperatures within this many degrees count as one: a regulated cooler holds its set
/// point to a few tenths of a degree, and dark current changes by about 12% per degree.
pub(crate) const TEMPERATURE_TOLERANCE: f64 = 1.0;

/// The exposure and sensor temperature a frame was taken under, as its source states them. Carried
/// with the frame's statistics, because the metadata they come from is dropped for every frame but
/// the first.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub(crate) struct CaptureConditions {
    /// Seconds — see [`ImageMetadata::exposure_time`].
    pub(crate) exposure_time: Option<f64>,
    /// Degrees Celsius — see [`ImageMetadata::ccd_temp`].
    pub(crate) ccd_temp: Option<f64>,
    /// Degrees Celsius — see [`ImageMetadata::camera_temp`].
    pub(crate) camera_temp: Option<f64>,
}

impl CaptureConditions {
    pub(crate) const fn of(metadata: &ImageMetadata) -> Self {
        Self {
            exposure_time: metadata.exposure_time,
            ccd_temp: metadata.ccd_temp,
            camera_temp: metadata.camera_temp,
        }
    }

    /// What every frame of `set` shares: each condition as the mean of the frames' values when
    /// every frame states one and they all agree, and `None` when a frame is silent or two
    /// disagree. One frame's value would describe that frame alone.
    pub(crate) fn shared(set: impl IntoIterator<Item = Self> + Clone) -> Self {
        Self {
            exposure_time: CaptureCondition::Exposure.shared(set.clone()),
            ccd_temp: CaptureCondition::Temperature.shared(set.clone()),
            camera_temp: CaptureCondition::CameraTemperature.shared(set),
        }
    }

    /// Whether every frame of `set` that states a condition agrees with every other one on it. The
    /// camera body's temperature is not held to it: a body warms through a session of darks while
    /// the sensor they measure is what matters, so a set whose bodies drifted only states none.
    ///
    /// # Errors
    ///
    /// The first frame, in index order, whose exposure and then whose temperature lies outside the
    /// tolerance of an earlier frame's, named beside the earlier frame at the far end of the range.
    pub(crate) fn check_agreement(
        set: impl IntoIterator<Item = Self> + Clone,
    ) -> Result<(), ConditionMismatch> {
        CaptureCondition::Exposure.check_agreement(set.clone())?;
        CaptureCondition::Temperature.check_agreement(set)
    }
}

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

    fn check_agreement(
        self,
        set: impl IntoIterator<Item = CaptureConditions>,
    ) -> Result<(), ConditionMismatch> {
        let mut range: Option<StatedRange> = None;
        for (index, conditions) in set.into_iter().enumerate() {
            let Some(value) = self.of(conditions) else {
                continue;
            };
            let Some(range) = &mut range else {
                range = Some(StatedRange::new(index, value));
                continue;
            };
            let (low, high) = (range.low.min(value), range.high.max(value));
            if !self.agree(low, high) {
                let (reference_index, reference) = if value > range.high {
                    (range.low_index, range.low)
                } else {
                    (range.high_index, range.high)
                };
                return Err(ConditionMismatch {
                    condition: self,
                    index,
                    value,
                    reference_index,
                    reference,
                });
            }
            range.include(index, value);
        }
        Ok(())
    }

    fn shared(self, set: impl IntoIterator<Item = CaptureConditions> + Clone) -> Option<f64> {
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
    low_index: usize,
    high: f64,
    high_index: usize,
}

impl StatedRange {
    const fn new(index: usize, value: f64) -> Self {
        Self {
            low: value,
            low_index: index,
            high: value,
            high_index: index,
        }
    }

    fn include(&mut self, index: usize, value: f64) {
        if value < self.low {
            self.low = value;
            self.low_index = index;
        }
        if value > self.high {
            self.high = value;
            self.high_index = index;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn conditions(exposure_time: Option<f64>, ccd_temp: Option<f64>) -> CaptureConditions {
        CaptureConditions {
            exposure_time,
            ccd_temp,
            camera_temp: None,
        }
    }

    #[test]
    fn a_set_shares_the_mean_of_values_that_agree() {
        // 300, 301 and 302.5 s span 2.5 s, inside 1% of 302.5 = 3.025 s: the mean is 903.5 / 3.
        // −10.0 and −9.5 °C span 0.5 °C: the mean is −9.75 °C.
        let set = [
            conditions(Some(300.0), Some(-10.0)),
            conditions(Some(301.0), Some(-9.5)),
            conditions(Some(302.5), Some(-9.75)),
        ];
        assert_eq!(CaptureConditions::check_agreement(set), Ok(()));
        assert_eq!(
            CaptureConditions::shared(set),
            conditions(Some(903.5 / 3.0), Some(-29.25 / 3.0))
        );
    }

    #[test]
    fn a_silent_or_disagreeing_frame_leaves_the_condition_unshared() {
        // One frame states no temperature: the exposures still agree and are shared.
        let silent = [
            conditions(Some(120.0), Some(0.0)),
            conditions(Some(120.0), None),
        ];
        assert_eq!(CaptureConditions::check_agreement(silent), Ok(()));
        assert_eq!(
            CaptureConditions::shared(silent),
            conditions(Some(120.0), None)
        );
        // Nothing stated at all: nothing to compare, nothing shared.
        let none = [conditions(None, None), conditions(None, None)];
        assert_eq!(CaptureConditions::check_agreement(none), Ok(()));
        assert_eq!(CaptureConditions::shared(none), conditions(None, None));
        // Camera bodies at 20.5 and 21 °C share 20.75; at 20.5 and 24 °C — a body warming through a
        // session of darks — they share none, and the set is not refused for it.
        let body = |camera_temp| CaptureConditions {
            camera_temp: Some(camera_temp),
            ..conditions(Some(120.0), None)
        };
        assert_eq!(
            CaptureConditions::shared([body(20.5), body(21.0)]).camera_temp,
            Some(20.75)
        );
        let warming = [body(20.5), body(24.0)];
        assert_eq!(CaptureConditions::check_agreement(warming), Ok(()));
        assert_eq!(CaptureConditions::shared(warming).camera_temp, None);
    }

    #[test]
    fn the_first_frame_out_of_range_is_named_against_the_far_end() {
        // 120 s then 300 s: 180 s apart, far outside 1% of 300 s. Frame 1 is the first out, and
        // the low end it is too far from is frame 0.
        let mixed = [
            conditions(Some(120.0), None),
            conditions(Some(300.0), None),
            conditions(Some(300.0), None),
        ];
        assert_eq!(
            CaptureConditions::check_agreement(mixed),
            Err(ConditionMismatch {
                condition: CaptureCondition::Exposure,
                index: 1,
                value: 300.0,
                reference_index: 0,
                reference: 120.0,
            })
        );
        assert_eq!(CaptureConditions::shared(mixed), conditions(None, None));
        // Temperatures −10.0, −9.4 and −10.5: the third widens the range to 1.1 °C, past 1 °C,
        // below the low end, so it is named against the high end, frame 1.
        let drifting = [
            conditions(None, Some(-10.0)),
            conditions(None, Some(-9.4)),
            conditions(None, Some(-10.5)),
        ];
        assert_eq!(
            CaptureConditions::check_agreement(drifting),
            Err(ConditionMismatch {
                condition: CaptureCondition::Temperature,
                index: 2,
                value: -10.5,
                reference_index: 1,
                reference: -9.4,
            })
        );
    }

    #[test]
    fn tolerances_are_inclusive_at_their_bound() {
        // Exposure: 99 and 100 s are exactly 1% of 100 s apart. Temperature: exactly 1 °C.
        assert!(CaptureCondition::Exposure.agree(99.0, 100.0));
        assert!(!CaptureCondition::Exposure.agree(98.9, 100.0));
        assert!(CaptureCondition::Temperature.agree(-11.0, -10.0));
        assert!(!CaptureCondition::Temperature.agree(-11.25, -10.0));
    }
}
