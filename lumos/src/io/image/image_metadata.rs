//! Observation metadata carried by every image product.

use std::sync::Arc;

use fits_well::image::SampleType;
use glam::DVec2;

use crate::io::image::calibration_state::CalibrationState;
use crate::io::image::flat_gain::FlatGain;
use crate::io::image::image_provenance::{DemosaicProvenance, ImageProvenance, RowOrder};
use crate::io::image::mosaic_noise::MosaicNoise;
use crate::io::image::pixel_flags::SATURATION_FRACTION;
use crate::io::image::sample_domain::SampleDomain;
use crate::io::image::unverified_conditions::UnverifiedConditions;
use crate::math::size2us::Size2us;

/// Metadata and provenance shared by sensor, linear, and preview image products.
#[derive(Debug, Clone, Default)]
pub struct ImageMetadata {
    pub object: Option<String>,
    pub instrument: Option<String>,
    pub telescope: Option<String>,
    pub date_obs: Option<String>,
    pub exposure_time: Option<f64>,
    pub iso: Option<u32>,
    /// The type a FITS source stored its samples as, after its unsigned-offset convention;
    /// `None` for a source with no such declaration.
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
    /// What one sample is worth in the source's own terms: the span its decoder divided by, the
    /// pedestal it still carries, and the unit.
    ///
    /// `None` for an image this crate synthesized rather than decoded, and for a preview raster
    /// that declared no domain. Two frames are commensurate when both answer and
    /// [`SampleDomain::conversion_to`] relates the answers; when either is `None` there is nothing
    /// to compare, which is not the same as agreeing. Calibration updates the pedestal when it
    /// subtracts a master.
    pub domain: Option<SampleDomain>,
    /// The uncertainty one quantization step adds to a sample, `step / √12`, in the samples' own
    /// units: the source's ADC step, a lower bound on any sample's noise before a flat divided it.
    ///
    /// Set by a decoder that knows the step — a RAW with a linear curve, an integer FITS — and by
    /// the combine for a master, which states the largest step of its inputs. Calibration leaves it
    /// as it is: subtracting a master adds no step to the light's own, and the flat's division
    /// scales it per pixel, which the noise model applies where it reads the flat. A demosaic
    /// clears it: interpolation mixes samples, so the bound no longer describes one of them.
    pub quantization_sigma: Option<f32>,
    /// The white noise of the mosaic a demosaic made this frame from, which a measurement of the
    /// frame would understate. Set by the demosaic; `None` for any frame not demosaiced, and for a
    /// master, which the combine made.
    pub mosaic_noise: Option<MosaicNoise>,
    /// The gain a flat applied to each pixel's value and noise, in this image's own pixels: set
    /// when calibration divides by a flat — shared by every light it divides — kept through a
    /// demosaic and through a FITS file of the mosaic (its `LUMGAIN` extension), and replaced by
    /// its warped grid when a warp moves the pixels. `None` for a frame no flat divided, and for a
    /// master, which the combine made.
    pub flat_gain: Option<Arc<FlatGain>>,
    /// Whether every saturated pixel is flagged in the image's flags. A RAW decode and a FITS with
    /// a `DATAMAX` flag them, and calibration flags them before it moves the samples; anything else
    /// leaves a consumer to test the samples itself.
    pub saturation_flagged: bool,
    /// The parts of the signal calibration removed: set by `CalibrationMasters::calibrate` on a
    /// light, and by a calibration stack that subtracts a master from each frame. It guards
    /// against removing a part twice, and travels with the frame through demosaic.
    pub calibration: CalibrationState,
    /// The conditions not compared when a master holding dark signal was taken from this frame,
    /// or from any frame of the stack it is: a light's dark, a flat's flat-dark.
    pub unverified_dark: UnverifiedConditions,
}

impl ImageMetadata {
    /// The metadata of this image's pixels moved by `to_source`, an output-to-source map onto an
    /// output of `size`: its flat gain moves with them.
    pub(crate) fn warped(
        mut self,
        to_source: impl Fn(DVec2) -> DVec2 + Sync,
        size: Size2us,
    ) -> Self {
        self.flat_gain = self
            .flat_gain
            .map(|gain| Arc::new(gain.warped(to_source, size)));
        self
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

    /// The level a sample saturates at when the decoder flagged nothing: [`SATURATION_FRACTION`] of
    /// its declared ceiling, `DATAMAX` in the normalized domain, or of that domain's 1 when it
    /// declares none. It describes the samples as decoded, before any calibration moves them.
    pub(crate) fn saturation_level(&self) -> f32 {
        SATURATION_FRACTION * self.data_max.map_or(1.0, |max| max as f32)
    }

    /// Whether these samples came out of a demosaic, and so carry its interpolation artifacts. A
    /// monochrome sensor's frame is copied straight through, with nothing interpolated.
    pub(crate) fn is_demosaiced(&self) -> bool {
        self.provenance
            .as_ref()
            .is_some_and(|provenance| provenance.demosaic != DemosaicProvenance::None)
    }
}
