//! [`CaptureConditions`]: the exposure and sensor temperature a frame was taken under.

use serde::{Deserialize, Serialize};

use crate::frame::capture_condition::CaptureCondition;
use crate::frame::error::ConditionMismatch;
use crate::image::metadata::image_metadata::ImageMetadata;

/// The exposure and sensor temperature a frame was taken under, as its source states them. Carried
/// with the frame's statistics, because the metadata they come from is dropped for every frame but
/// the first.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub(crate) struct CaptureConditions {
    /// Seconds — see [`ObservationHeader::exposure_time`](crate::ObservationHeader::exposure_time).
    pub(crate) exposure_time: Option<f64>,
    /// Degrees Celsius — see [`ObservationHeader::ccd_temp`](crate::ObservationHeader::ccd_temp).
    pub(crate) ccd_temp: Option<f64>,
    /// Degrees Celsius — see
    /// [`ObservationHeader::camera_temp`](crate::ObservationHeader::camera_temp).
    pub(crate) camera_temp: Option<f64>,
}

impl CaptureConditions {
    pub(crate) const fn of(metadata: &ImageMetadata) -> Self {
        Self {
            exposure_time: metadata.header.exposure_time,
            ccd_temp: metadata.header.ccd_temp,
            camera_temp: metadata.header.camera_temp,
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

    /// Whether every frame of `set` that states an exposure agrees with every other one on it.
    ///
    /// # Errors
    ///
    /// The first frame, in index order, whose exposure lies outside the tolerance of an earlier
    /// frame's, named beside the earlier frame at the far end of the range.
    pub(crate) fn check_exposures(
        set: impl IntoIterator<Item = Self> + Clone,
    ) -> Result<(), ConditionMismatch> {
        CaptureCondition::Exposure.check_agreement(set)
    }

    /// The first frame of `set` whose sensor temperature lies outside the tolerance of an earlier
    /// frame's: not a refusal, since no reference refuses on temperature, but a set of which
    /// [`Self::shared`] states no temperature. The camera body's is not compared: a body warms
    /// through a session of darks while the sensor they measure is what matters.
    pub(crate) fn temperature_disagreement(
        set: impl IntoIterator<Item = Self> + Clone,
    ) -> Option<ConditionMismatch> {
        CaptureCondition::Temperature.check_agreement(set).err()
    }
}

#[cfg(test)]
mod tests;
