//! [`ObservationHeader`]: what a source stated about the observation and the camera.

use fits_well::image::SampleType;
use serde::{Deserialize, Serialize};

use crate::image::quality_flags::SATURATION_FRACTION;
use crate::image::sample_type_record::SampleTypeRecord;

/// What a source stated about the observation and the camera: facts of the exposure, which no step
/// that changes the samples changes. The combine states a stack's own capture conditions in it.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ObservationHeader {
    pub object: Option<String>,
    /// The camera: a FITS `INSTRUME`, or a camera RAW's make and model.
    pub instrument: Option<String>,
    pub telescope: Option<String>,
    /// When the frame was taken, as ISO 8601 text in UTC unless the source says otherwise: a FITS
    /// `DATE-OBS`.
    pub date_obs: Option<String>,
    /// When the frame was taken by a clock whose zone the source does not state, as ISO 8601 text:
    /// a FITS `DATE-LOC`, or a camera RAW's own clock, set to whatever zone its owner chose. Not
    /// [`Self::date_obs`], which a reader takes for UTC.
    pub date_local: Option<String>,
    pub exposure_time: Option<f64>,
    pub iso: Option<u32>,
    /// The type a FITS source stored its samples as, after its unsigned-offset convention;
    /// `None` for a source with no such declaration.
    #[serde(
        serialize_with = "SampleTypeRecord::serialize_option",
        deserialize_with = "SampleTypeRecord::deserialize_option"
    )]
    pub sample_type: Option<SampleType>,
    /// The multipliers that balance the samples as stored, `[R, G1, B, G2]`, normalized so the
    /// smallest is `1.0`: the camera's as-shot balance, or unity for a RAW file whose samples
    /// already carry it (LibRaw's `as_shot_wb_applied`). X-Trans and RAW metadata without a second
    /// green duplicate `G1`.
    ///
    /// The samples keep the sensor's balance: the demosaic balances by these before it
    /// interpolates, as its direction decisions assume balanced channels, and divides them out
    /// after.
    pub camera_white_balance: Option<[f32; 4]>,
    /// Filter name (e.g. "Ha", "OIII", "L", "R"). Critical for narrowband.
    pub filter: Option<String>,
    /// Camera gain setting (unitless, camera-specific).
    pub gain: Option<f64>,
    /// Electrons per ADU (e-/ADU). Used for noise modeling.
    pub egain: Option<f64>,
    /// CCD/sensor temperature in degrees Celsius during exposure.
    pub ccd_temp: Option<f64>,
    /// The camera body's temperature in degrees Celsius, from a camera RAW's maker notes: warmer
    /// than the sensor and lagging it, so a dark is matched on it only where no sensor temperature
    /// is stated.
    pub camera_temp: Option<f64>,
    /// Frame type: "Light", "Dark", "Flat", "Bias", etc.
    pub image_type: Option<String>,
    /// Horizontal binning factor.
    pub xbinning: Option<i32>,
    /// Vertical binning factor.
    pub ybinning: Option<i32>,
    /// Target sensor temperature setpoint in degrees Celsius.
    pub set_temp: Option<f64>,
    /// Camera offset setting (unitless, camera-specific).
    pub offset: Option<i32>,
    /// Focal length in mm.
    pub focal_length: Option<f64>,
    /// Airmass at time of observation.
    pub airmass: Option<f64>,
    /// Right ascension of telescope pointing in degrees.
    pub ra_deg: Option<f64>,
    /// Declination of telescope pointing in degrees.
    pub dec_deg: Option<f64>,
    /// Pixel size in microns (X axis).
    pub pixel_size_x: Option<f64>,
    /// Pixel size in microns (Y axis).
    pub pixel_size_y: Option<f64>,
    /// Maximum valid pixel value (saturation level).
    pub data_max: Option<f64>,
}

impl ObservationHeader {
    /// The level a sample saturates at when the decoder flagged nothing: [`SATURATION_FRACTION`] of
    /// its declared ceiling, `DATAMAX` in the normalized domain, or of that domain's 1 when it
    /// declares none. It describes the samples as decoded, before any calibration moves them.
    pub(crate) fn saturation_level(&self) -> f32 {
        SATURATION_FRACTION * self.data_max.map_or(1.0, |max| max as f32)
    }
}
