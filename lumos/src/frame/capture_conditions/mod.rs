//! [`CaptureConditions`]: the exposure, temperatures and sensor setting a frame was taken under.

use serde::{Deserialize, Serialize};

use crate::frame::capture_condition::CaptureCondition;
use crate::frame::error::{ConditionMismatch, SettingMismatch};
use crate::image::metadata::image_metadata::ImageMetadata;

/// The exposure, sensor and camera temperatures and the sensor setting a frame was taken under, as
/// its source states them. Carried with the frame's statistics, because the metadata they come
/// from is dropped for every frame but the first.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub(crate) struct CaptureConditions {
    /// Seconds — see [`ObservationHeader::exposure_time`](crate::ObservationHeader::exposure_time).
    pub(crate) exposure_time: Option<f64>,
    /// Degrees Celsius — see [`ObservationHeader::ccd_temp`](crate::ObservationHeader::ccd_temp).
    pub(crate) ccd_temp: Option<f64>,
    /// Degrees Celsius — see
    /// [`ObservationHeader::camera_temp`](crate::ObservationHeader::camera_temp).
    pub(crate) camera_temp: Option<f64>,
    pub(crate) setting: SensorSetting,
}

/// How the sensor was set to read a frame: what a bias and a dark must share with the frame they
/// calibrate, since each changes the offset, the read noise or the dark current per pixel, and what
/// the frames of one master must share. Each is compared by equality where both sides state it.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub(crate) struct SensorSetting {
    /// See [`ObservationHeader::gain`](crate::ObservationHeader::gain).
    pub(crate) gain: Option<f64>,
    /// See [`ObservationHeader::iso`](crate::ObservationHeader::iso).
    pub(crate) iso: Option<u32>,
    /// See [`ObservationHeader::offset`](crate::ObservationHeader::offset).
    pub(crate) offset: Option<i32>,
    /// See [`ObservationHeader::xbinning`](crate::ObservationHeader::xbinning).
    pub(crate) xbinning: Option<i32>,
    /// See [`ObservationHeader::ybinning`](crate::ObservationHeader::ybinning).
    pub(crate) ybinning: Option<i32>,
}

/// What comparing a master's setting with a frame's found, short of a difference.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SettingsCompared {
    /// Every setting either side states, both state, and at least one is stated.
    All,
    /// A setting one side states and the other does not, or none stated: not verified.
    Partly,
}

impl SensorSetting {
    pub(crate) const fn of(metadata: &ImageMetadata) -> Self {
        let header = &metadata.header;
        Self {
            gain: header.gain,
            iso: header.iso,
            offset: header.offset,
            xbinning: header.xbinning,
            ybinning: header.ybinning,
        }
    }

    /// What every frame of `set` shares: each setting where every frame states it and all agree.
    fn shared(set: impl IntoIterator<Item = CaptureConditions>) -> Self {
        fn agreed<T: PartialEq + Copy>(values: impl IntoIterator<Item = Option<T>>) -> Option<T> {
            let mut values = values.into_iter();
            let first = values.next()??;
            values.all(|value| value == Some(first)).then_some(first)
        }
        let settings: Vec<Self> = set
            .into_iter()
            .map(|conditions| conditions.setting)
            .collect();
        let set = || settings.iter();
        Self {
            gain: agreed(set().map(|setting| setting.gain)),
            iso: agreed(set().map(|setting| setting.iso)),
            offset: agreed(set().map(|setting| setting.offset)),
            xbinning: agreed(set().map(|setting| setting.xbinning)),
            ybinning: agreed(set().map(|setting| setting.ybinning)),
        }
    }

    /// Compare the binning of a master taken at `master` with a frame's taken at `self`: a flat
    /// must share the light's photosites, and not its gain or offset, which a flat divides out.
    ///
    /// # Errors
    ///
    /// The first axis both state and differ on.
    pub(crate) fn compare_binning(self, master: Self) -> Result<(), SettingMismatch> {
        Self {
            gain: None,
            iso: None,
            offset: None,
            ..self
        }
        .compare(Self {
            gain: None,
            iso: None,
            offset: None,
            ..master
        })
        .map(|_| ())
    }

    /// Compare a master taken at `master` with a frame taken at `self`.
    ///
    /// # Errors
    ///
    /// The first setting, in [`CaptureCondition::SETTINGS`] order, both state and differ on.
    pub(crate) fn compare(self, master: Self) -> Result<SettingsCompared, SettingMismatch> {
        let frame = CaptureConditions {
            setting: self,
            ..CaptureConditions::default()
        };
        let master = CaptureConditions {
            setting: master,
            ..CaptureConditions::default()
        };
        let mut compared = SettingsCompared::All;
        let mut stated = false;
        for condition in CaptureCondition::SETTINGS {
            match (condition.of(frame), condition.of(master)) {
                (Some(frame), Some(master)) if frame != master => {
                    return Err(SettingMismatch {
                        condition,
                        frame,
                        master,
                    });
                }
                (Some(_), Some(_)) => stated = true,
                (None, None) => {}
                (Some(_), None) | (None, Some(_)) => compared = SettingsCompared::Partly,
            }
        }
        Ok(if stated {
            compared
        } else {
            SettingsCompared::Partly
        })
    }
}

impl CaptureConditions {
    pub(crate) const fn of(metadata: &ImageMetadata) -> Self {
        Self {
            exposure_time: metadata.header.exposure_time,
            ccd_temp: metadata.header.ccd_temp,
            camera_temp: metadata.header.camera_temp,
            setting: SensorSetting::of(metadata),
        }
    }

    /// What every frame of `set` shares: each condition as the mean of the frames' values when
    /// every frame states one and they all agree, and `None` when a frame is silent or two
    /// disagree. One frame's value would describe that frame alone.
    pub(crate) fn shared(set: impl IntoIterator<Item = Self> + Clone) -> Self {
        Self {
            exposure_time: CaptureCondition::Exposure.shared(set.clone()),
            ccd_temp: CaptureCondition::Temperature.shared(set.clone()),
            camera_temp: CaptureCondition::CameraTemperature.shared(set.clone()),
            setting: SensorSetting::shared(set),
        }
    }

    /// Whether the frames of `set` agree on each setting they state.
    ///
    /// # Errors
    ///
    /// The first setting, in [`CaptureCondition::SETTINGS`] order, on which a frame differs from an
    /// earlier one.
    pub(crate) fn check_settings(
        set: impl IntoIterator<Item = Self>,
    ) -> Result<(), ConditionMismatch> {
        let set: Vec<Self> = set.into_iter().collect();
        CaptureCondition::SETTINGS
            .into_iter()
            .try_for_each(|condition| condition.check_agreement(set.iter().copied()))
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
