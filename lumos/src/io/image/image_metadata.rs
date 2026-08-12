//! Observation metadata carried by every image product.
//!
//! [`BitPix`] lives here rather than in its own file: it is only ever reached as
//! [`ImageMetadata::bitpix`], the pixel type the FITS header declared.

use crate::io::image::cfa;
use crate::io::image::image_provenance::{DemosaicProvenance, ImageProvenance, RowOrder};
use crate::io::image::sample_domain::SampleDomain;

/// FITS BITPIX values representing pixel data types.
///
/// FITS natively supports only signed integers. Unsigned integers use the
/// BZERO convention (e.g., BITPIX=16 + BZERO=32768 for unsigned 16-bit).
/// fits-well's `SampleType` resolves this and reports the effective type.
/// The unsigned variants here preserve the distinction for correct normalization.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum BitPix {
    #[default]
    UInt8,
    Int16,
    UInt16,
    Int32,
    UInt32,
    Int64,
    Float32,
    Float64,
}

impl BitPix {
    /// Whether samples are stored as integers rather than IEEE floats.
    ///
    /// The distinction two decode decisions turn on, which is why it is named once rather than
    /// spelled as a six-variant match at each: an integer `BITPIX` has an exact ADC step to derive a
    /// quantization sigma from, and it carries its undefined samples as a declared `BLANK` value
    /// instead of in-band NaN. A variant added later cannot then be missed by one and not the other.
    pub(crate) fn is_integer(self) -> bool {
        match self {
            Self::UInt8 | Self::Int16 | Self::UInt16 | Self::Int32 | Self::UInt32 | Self::Int64 => {
                true
            }
            Self::Float32 | Self::Float64 => false,
        }
    }
}

/// Metadata and provenance shared by sensor, linear, and preview image products.
#[derive(Debug, Clone, Default)]
pub struct ImageMetadata {
    pub object: Option<String>,
    pub instrument: Option<String>,
    pub telescope: Option<String>,
    pub date_obs: Option<String>,
    pub exposure_time: Option<f64>,
    pub iso: Option<u32>,
    pub bitpix: BitPix,
    pub header_dimensions: Vec<usize>,
    /// CFA sensor type, if the image originated from a raw sensor.
    /// `None` for non-CFA sources (FITS, monochrome sensors).
    pub cfa_type: Option<cfa::CfaType>,
    /// Camera-recorded white-balance multipliers `[R, G1, B, G2]`, normalized so the smallest
    /// multiplier is `1.0`. X-Trans and RAW metadata without a second green duplicate `G1`.
    ///
    /// Metadata only: RAW decoding and calibration keep unity white balance.
    pub camera_white_balance: Option<[f32; 4]>,
    /// Filter name (e.g. "Ha", "OIII", "L", "R"). Critical for narrowband.
    pub filter: Option<String>,
    /// Camera gain setting (unitless, camera-specific).
    pub gain: Option<f64>,
    /// Electrons per ADU (e-/ADU). Used for noise modeling.
    pub egain: Option<f64>,
    /// CCD/sensor temperature in degrees Celsius during exposure.
    pub ccd_temp: Option<f64>,
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
    pub provenance: Option<ImageProvenance>,
    /// Set by `CalibrationMasters::calibrate` — guards against applying the dark/flat twice
    /// (the FITS `CALSTAT` convention). Travels with the frame through demosaic.
    pub calibrated: bool,
}

impl ImageMetadata {
    /// What one sample is worth in the source's own terms — the span its decoder divided by, and
    /// the unit that span was in.
    ///
    /// `None` for an image this crate synthesized rather than decoded, and for a preview raster that
    /// declared no domain. Two frames are commensurate when both answer and the answers satisfy
    /// [`SampleDomain::commensurate_with`]; when either is `None` there is nothing to compare, which
    /// is not the same as agreeing.
    pub fn sample_domain(&self) -> Option<SampleDomain> {
        self.provenance
            .as_ref()
            .and_then(|provenance| provenance.transfer.sample_domain())
    }

    /// Which end of the image the first stored row belongs to, or `None` for an image this crate
    /// synthesized rather than decoded.
    ///
    /// Two frames are the same view only when both answer and the answers match; `None` is "cannot
    /// tell", which is not the same as agreeing.
    pub fn row_order(&self) -> Option<RowOrder> {
        self.provenance
            .as_ref()
            .map(|provenance| provenance.row_order)
    }

    /// Whether these samples came out of a demosaic, and so carry its interpolation artifacts.
    ///
    /// Not `cfa_type.is_some()`: that records which sensor pattern the frame came from and stays
    /// set on a monochrome frame, which is copied straight through with nothing interpolated.
    pub(crate) fn is_demosaiced(&self) -> bool {
        self.provenance
            .as_ref()
            .is_some_and(|provenance| provenance.demosaic != DemosaicProvenance::None)
    }
}
