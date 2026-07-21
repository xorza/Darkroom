pub(crate) mod cfa;
pub(crate) mod error;
pub(crate) mod fits;
pub(crate) mod linear;
pub(crate) mod linear_pixels;
pub(crate) mod sensor;
#[cfg(test)]
mod synthetic_tests;

use common::CancelToken;
use imaginarium::{ChannelCount, ColorFormat, Image};
use std::path::Path;

use crate::io::image::error::ImageError;
use crate::io::image::fits::decode as fits_decode;
use crate::io::image::fits::options::FitsLoadOptions;
use crate::io::image::fits::provenance::FitsTransferProvenance;
use crate::io::image::linear::LinearImage;
use crate::io::raw;
use crate::math::vec2us::Vec2us;
use crate::resources;

const FITS_EXTENSIONS: &[&str] = &["fits", "fit"];
const STANDARD_IMAGE_EXTENSIONS: &[&str] = &["tiff", "tif", "png", "jpg", "jpeg"];

/// Every file extension accepted by [`PreviewImage::from_file`].
pub const PREVIEW_IMAGE_EXTENSIONS: &[&str] = &[
    "fits", "fit", "raf", "cr2", "cr3", "nef", "arw", "dng", "tiff", "tif", "png", "jpg", "jpeg",
];

/// Cancellation, resource controls, and format policy shared by file decoders.
#[derive(Debug, Clone)]
pub struct LoadContext {
    /// Cooperative cancellation token polled between bounded decode stages.
    pub cancel: CancelToken,
    /// FITS source, output, and estimated peak byte ceiling.
    pub memory_limit_bytes: u64,
    /// FITS-specific policy; ignored by non-FITS decoders.
    pub fits: FitsLoadOptions,
}

impl LoadContext {
    /// Creates a context with strict FITS defaults and the supplied resource controls.
    pub fn new(cancel: CancelToken, memory_limit_bytes: u64) -> Self {
        Self {
            cancel,
            memory_limit_bytes,
            fits: FitsLoadOptions::default(),
        }
    }

    pub(crate) fn check_cancelled(&self, path: &Path) -> Result<(), ImageError> {
        if self.cancel.is_cancelled() {
            return Err(ImageError::Cancelled {
                path: path.to_path_buf(),
            });
        }
        Ok(())
    }
}

impl Default for LoadContext {
    fn default() -> Self {
        Self::new(
            CancelToken::never(),
            resources::memory_budget(resources::available_memory()),
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceContainer {
    Fits,
    CameraRaw,
    Tiff,
    Png,
    Jpeg,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DecoderProvenance {
    FitsWell,
    LibRaw,
    Imaginarium,
}

#[derive(Debug, Clone, PartialEq)]
pub enum TransferProvenance {
    FitsPhysical(FitsTransferProvenance),
    RawNormalized,
    DeclaredLinearRaster,
    UnspecifiedRaster,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColorProvenance {
    SensorCfa,
    SensorRgb,
    Monochrome,
    Unspecified,
    UnmanagedRaster { alpha_dropped: bool },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DemosaicProvenance {
    None,
    LumosRcd,
    LumosMarkesteijn,
    LibRaw,
}

/// Decoder decisions that affect the meaning of the returned samples.
#[derive(Debug, Clone, PartialEq)]
pub struct ImageProvenance {
    pub container: SourceContainer,
    pub decoder: DecoderProvenance,
    pub transfer: TransferProvenance,
    pub color: ColorProvenance,
    /// Whether this load path itself clipped samples.
    pub clipped: bool,
    pub demosaic: DemosaicProvenance,
}

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

/// Image dimensions: pixel size and number of channels.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ImageDimensions {
    size: Vec2us,
    channels: usize,
}

impl ImageDimensions {
    pub fn new(size: impl Into<Vec2us>, channels: usize) -> Self {
        let size = size.into();
        assert!(size.x > 0, "Width must be positive");
        assert!(size.y > 0, "Height must be positive");
        assert!(
            channels == 1 || channels == 3,
            "Only 1 (grayscale) or 3 (RGB) channels supported, got {}",
            channels
        );
        let pixel_count = size
            .x
            .checked_mul(size.y)
            .expect("Image pixel count must fit in usize");
        pixel_count
            .checked_mul(channels)
            .expect("Image sample count must fit in usize");
        Self { size, channels }
    }

    /// Pixel dimensions as a `(width, height)` vector.
    pub fn size(&self) -> Vec2us {
        self.size
    }

    pub fn width(&self) -> usize {
        self.size.x
    }

    pub fn height(&self) -> usize {
        self.size.y
    }

    pub fn channels(&self) -> usize {
        self.channels
    }

    /// Total number of f32 samples: `width * height * channels`.
    /// For a 100x100 RGB image, returns 30000.
    pub fn sample_count(&self) -> usize {
        self.pixel_count()
            .checked_mul(self.channels)
            .expect("ImageDimensions validates sample count during construction")
    }

    /// Number of pixels: `width * height`.
    /// For a 100x100 RGB image, returns 10000.
    pub fn pixel_count(&self) -> usize {
        self.size
            .x
            .checked_mul(self.size.y)
            .expect("ImageDimensions validates pixel count during construction")
    }

    pub fn is_grayscale(&self) -> bool {
        self.channels == 1
    }

    pub fn is_rgb(&self) -> bool {
        self.channels == 3
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

/// A decoded display or inspection product that cannot enter the scientific pipeline directly.
#[derive(Debug)]
pub struct PreviewImage {
    pub metadata: ImageMetadata,
    image: Image,
}

fn file_extension(path: &Path) -> String {
    path.extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or("")
        .to_ascii_lowercase()
}

fn scientific_rejection(path: &Path, reason: impl Into<String>) -> ImageError {
    ImageError::ScientificInputRejected {
        path: path.to_path_buf(),
        reason: reason.into(),
    }
}

fn read_standard_image(path: &Path) -> Result<Image, ImageError> {
    Image::read_file(path).map_err(|source| ImageError::Image {
        path: path.to_path_buf(),
        source,
    })
}

fn standard_container(extension: &str) -> SourceContainer {
    match extension {
        "tiff" | "tif" => SourceContainer::Tiff,
        "png" => SourceContainer::Png,
        "jpg" | "jpeg" => SourceContainer::Jpeg,
        _ => unreachable!("standard extension was validated before selecting its container"),
    }
}

impl PreviewImage {
    /// Load a display or inspection image from FITS, camera RAW, TIFF, PNG, or JPEG.
    pub fn from_file<P: AsRef<Path>>(
        path: P,
        context: &LoadContext,
    ) -> Result<Self, ImageError> {
        let path = path.as_ref();
        context.check_cancelled(path)?;
        let extension = file_extension(path);

        if FITS_EXTENSIONS.contains(&extension.as_str()) {
            return fits_decode::load_preview_fits(path, context).map(Into::into);
        }

        if raw::RAW_EXTENSIONS.contains(&extension.as_str()) {
            return raw::load_raw(path, &context.cancel).map(Into::into);
        }

        if STANDARD_IMAGE_EXTENSIONS.contains(&extension.as_str()) {
            let decoded = read_standard_image(path)?;
            context.check_cancelled(path)?;
            let alpha_dropped = decoded.desc().color_format.channel_count == ChannelCount::Rgba;
            let target = f32_target_format(&decoded);
            let image = decoded
                .convert(target)
                .expect("standard image converts to its f32 channel format");
            let metadata = ImageMetadata {
                provenance: Some(ImageProvenance {
                    container: standard_container(&extension),
                    decoder: DecoderProvenance::Imaginarium,
                    transfer: TransferProvenance::UnspecifiedRaster,
                    color: ColorProvenance::UnmanagedRaster { alpha_dropped },
                    clipped: false,
                    demosaic: DemosaicProvenance::None,
                }),
                ..Default::default()
            };
            return Ok(Self { metadata, image });
        }

        Err(ImageError::UnsupportedFormat { extension })
    }
}

impl From<LinearImage> for PreviewImage {
    fn from(linear: LinearImage) -> Self {
        let image = Image::from(&linear);
        Self {
            metadata: linear.metadata,
            image,
        }
    }
}

impl From<PreviewImage> for Image {
    fn from(preview: PreviewImage) -> Self {
        preview.image
    }
}

/// The `f32` target format a given image deinterleaves into: `L_F32` for
/// grayscale, `RGB_F32` for color.
fn f32_target_format(image: &Image) -> ColorFormat {
    match image.desc().color_format.channel_count {
        ChannelCount::L => ColorFormat::L_F32,
        ChannelCount::Rgb | ChannelCount::Rgba => ColorFormat::RGB_F32,
    }
}

#[cfg(test)]
mod tests;
