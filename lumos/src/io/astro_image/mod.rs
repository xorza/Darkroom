pub(crate) mod cfa;
pub(crate) mod error;
mod fits;
pub(crate) mod sensor;
#[cfg(test)]
mod synthetic_tests;

use rayon::prelude::*;

use error::ImageError;

use imaginarium::{ChannelCount, ChannelType, ColorFormat, Image};
use std::ops::SubAssign;
use std::path::Path;

#[cfg(test)]
use common::Rgb;
use common::Vec2us;
use imaginarium::{Buffer2, DeinterleavedImageData};

use crate::io::raw;
use crate::math::sum::sum_f32;
use crate::stacking::frame_store::StackableImage;

const FITS_EXTENSIONS: &[&str] = &["fits", "fit"];
const STANDARD_IMAGE_EXTENSIONS: &[&str] = &["tiff", "tif", "png", "jpg", "jpeg"];

/// Every file extension accepted by [`AstroImage::from_file`].
pub const ASTRO_IMAGE_EXTENSIONS: &[&str] = &[
    "fits", "fit", "raf", "cr2", "cr3", "nef", "arw", "dng", "tiff", "tif", "png", "jpg", "jpeg",
];

/// FITS BITPIX values representing pixel data types.
///
/// FITS natively supports only signed integers. Unsigned integers use the
/// BZERO convention (e.g., BITPIX=16 + BZERO=32768 for unsigned 16-bit).
/// fits-well's `SampleType` resolves this and reports the effective type.
/// The unsigned variants here preserve the distinction for correct normalization.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, Default, serde::Serialize, serde::Deserialize,
)]
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
    /// Normalization divisor for converting integer FITS data to [0,1].
    /// Returns `None` for float types (assumed already in correct range).
    pub fn normalization_max(self) -> Option<f32> {
        match self {
            BitPix::UInt8 => Some(255.0),
            BitPix::Int16 => Some(32767.0),
            BitPix::UInt16 => Some(65535.0),
            BitPix::Int32 => Some(2_147_483_647.0),
            BitPix::UInt32 => Some(4_294_967_295.0),
            BitPix::Int64 => Some(i64::MAX as f32),
            BitPix::Float32 | BitPix::Float64 => None,
        }
    }
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

/// Metadata extracted from FITS file headers or RAW EXIF.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct AstroImageMetadata {
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
    /// Set by `CalibrationMasters::calibrate` — guards against applying the dark/flat twice
    /// (the FITS `CALSTAT` convention). Travels with the frame through demosaic.
    pub calibrated: bool,
}

/// Pixel data storage - planar format for efficient per-channel operations.
#[derive(Debug, Clone)]
pub(crate) enum PixelData {
    L(DeinterleavedImageData<1, f32>),
    Rgb(DeinterleavedImageData<3, f32>),
}

impl PixelData {
    pub(crate) fn new_default(width: usize, height: usize, channels: usize) -> Self {
        match channels {
            1 => PixelData::L(DeinterleavedImageData::new_zeroed(width, height)),
            3 => PixelData::Rgb(DeinterleavedImageData::new_zeroed(width, height)),
            _ => panic!("Only 1 or 3 channels supported, got {channels}"),
        }
    }

    pub(crate) fn channel(&self, c: usize) -> &Buffer2<f32> {
        match self {
            PixelData::L(img) => {
                assert!(c == 0, "Grayscale image only has channel 0, got {c}");
                &img.channels[0]
            }
            PixelData::Rgb(img) => {
                assert!(c < 3, "RGB image has channels 0-2, got {c}");
                &img.channels[c]
            }
        }
    }

    pub(crate) fn channel_mut(&mut self, c: usize) -> &mut Buffer2<f32> {
        match self {
            PixelData::L(img) => {
                assert!(c == 0, "Grayscale image only has channel 0, got {c}");
                &mut img.channels[0]
            }
            PixelData::Rgb(img) => {
                assert!(c < 3, "RGB image has channels 0-2, got {c}");
                &mut img.channels[c]
            }
        }
    }

    pub(crate) fn channels(&self) -> usize {
        match self {
            PixelData::L(_) => 1,
            PixelData::Rgb(_) => 3,
        }
    }

    pub(crate) fn into_l(self) -> Buffer2<f32> {
        match self {
            PixelData::L(img) => {
                let [data] = img.channels;
                data
            }
            PixelData::Rgb(_) => panic!("Expected L variant, got Rgb"),
        }
    }
}

/// Represents an astronomical image.
#[derive(Debug, Clone)]
pub struct AstroImage {
    pub metadata: AstroImageMetadata,
    pub(crate) dimensions: ImageDimensions,
    pub(crate) pixels: PixelData,
}

impl AstroImage {
    /// Load an astronomical image from a file.
    ///
    /// Supported formats:
    /// - FITS: .fit, .fits
    /// - RAW: .raf, .cr2, .cr3, .nef, .arw, .dng
    /// - Standard: .tiff, .tif, .png, .jpg, .jpeg
    ///
    /// **Linearity:** the pipeline assumes flux-linear (photon-proportional) pixels. FITS and RAW
    /// satisfy this; standard formats are loaded as-is and *assumed* linear. PNG/JPEG and 8-bit
    /// TIFF are typically sRGB-gamma encoded — **not** valid input for calibration/stacking/
    /// photometry — so loading one logs a warning.
    pub fn from_file<P: AsRef<Path>>(path: P) -> Result<Self, ImageError> {
        let path = path.as_ref();
        let ext = path
            .extension()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_lowercase();

        if FITS_EXTENSIONS.contains(&ext.as_str()) {
            fits::load_fits(path)
        } else if raw::RAW_EXTENSIONS.contains(&ext.as_str()) {
            raw::load_raw(path)
        } else if STANDARD_IMAGE_EXTENSIONS.contains(&ext.as_str()) {
            let image = Image::read_file(path).map_err(|e| ImageError::Image {
                path: path.to_path_buf(),
                source: e,
            })?;
            // Float TIFFs hold linear scientific data and are valid input; integer/8-bit standard
            // formats (PNG/JPEG, 8-bit TIFF) are usually sRGB-gamma encoded, which corrupts the
            // linear-domain pipeline. Only the latter is worth warning about.
            if image.desc.color_format.channel_type != ChannelType::Float {
                tracing::warn!(
                    path = %path.display(),
                    format = %ext,
                    "loading a non-float standard image as linear; PNG/JPEG and 8-bit/integer \
                     TIFF are usually sRGB-gamma encoded — non-linear input corrupts \
                     calibration, stacking, and photometry"
                );
            }
            Ok(image.into())
        } else {
            Err(ImageError::UnsupportedFormat { extension: ext })
        }
    }

    /// Create from dimensions and interleaved pixel data (RGBRGBRGB...).
    pub fn from_pixels(dimensions: ImageDimensions, pixels: Vec<f32>) -> Self {
        assert_eq!(
            pixels.len(),
            dimensions.sample_count(),
            "Sample count mismatch: expected {}, got {}",
            dimensions.sample_count(),
            pixels.len()
        );

        let width = dimensions.size.x;
        let height = dimensions.size.y;
        let pixel_data = if dimensions.is_grayscale() {
            PixelData::L(DeinterleavedImageData::from_channels([Buffer2::new(
                width, height, pixels,
            )]))
        } else {
            let mut r: Buffer2<f32> = Buffer2::new_default(width, height);
            let mut g: Buffer2<f32> = Buffer2::new_default(width, height);
            let mut b: Buffer2<f32> = Buffer2::new_default(width, height);
            deinterleave_rgb(&pixels, r.pixels_mut(), g.pixels_mut(), b.pixels_mut());
            PixelData::Rgb(DeinterleavedImageData::from_channels([r, g, b]))
        };

        AstroImage {
            metadata: AstroImageMetadata::default(),
            dimensions,
            pixels: pixel_data,
        }
    }

    /// Create from planar channel data ([R, G, B] or single channel).
    pub fn from_planar_channels(
        dimensions: ImageDimensions,
        channels: impl IntoIterator<Item = Vec<f32>>,
    ) -> Self {
        let expected_pixels_per_channel = dimensions.size.x * dimensions.size.y;
        let width = dimensions.size.x;
        let height = dimensions.size.y;

        let mut iter = channels.into_iter();
        let pixel_data = if dimensions.is_grayscale() {
            let ch = iter.next().expect("Expected 1 channel for grayscale");
            assert_eq!(
                ch.len(),
                expected_pixels_per_channel,
                "Channel 0 pixel count mismatch"
            );
            assert!(iter.next().is_none(), "Too many channels for grayscale");
            PixelData::L(DeinterleavedImageData::from_channels([Buffer2::new(
                width, height, ch,
            )]))
        } else {
            let r = iter.next().expect("Expected 3 channels for RGB");
            let g = iter.next().expect("Expected 3 channels for RGB");
            let b = iter.next().expect("Expected 3 channels for RGB");
            assert_eq!(
                r.len(),
                expected_pixels_per_channel,
                "R channel pixel count mismatch"
            );
            assert_eq!(
                g.len(),
                expected_pixels_per_channel,
                "G channel pixel count mismatch"
            );
            assert_eq!(
                b.len(),
                expected_pixels_per_channel,
                "B channel pixel count mismatch"
            );
            assert!(iter.next().is_none(), "Too many channels for RGB");
            PixelData::Rgb(DeinterleavedImageData::from_channels([
                Buffer2::new(width, height, r),
                Buffer2::new(width, height, g),
                Buffer2::new(width, height, b),
            ]))
        };

        AstroImage {
            metadata: AstroImageMetadata::default(),
            dimensions,
            pixels: pixel_data,
        }
    }

    pub fn width(&self) -> usize {
        self.dimensions.size.x
    }

    pub fn height(&self) -> usize {
        self.dimensions.size.y
    }

    pub fn channels(&self) -> usize {
        self.dimensions.channels
    }

    pub fn dimensions(&self) -> ImageDimensions {
        self.dimensions
    }

    pub fn pixel_count(&self) -> usize {
        self.dimensions.pixel_count()
    }

    pub fn sample_count(&self) -> usize {
        self.dimensions.sample_count()
    }

    pub fn is_grayscale(&self) -> bool {
        self.dimensions.is_grayscale()
    }

    pub fn is_rgb(&self) -> bool {
        self.dimensions.is_rgb()
    }

    /// Get channel as Buffer2 reference (0=L or R, 1=G, 2=B).
    pub fn channel(&self, c: usize) -> &Buffer2<f32> {
        self.pixels.channel(c)
    }

    /// Get channel as mutable Buffer2 reference.
    pub fn channel_mut(&mut self, c: usize) -> &mut Buffer2<f32> {
        self.pixels.channel_mut(c)
    }

    /// Calculate mean pixel value across all channels.
    pub fn mean(&self) -> f32 {
        fn parallel_sum(values: &[f32]) -> f32 {
            values.par_chunks(8192).map(sum_f32).sum()
        }

        match &self.pixels {
            PixelData::L(img) => {
                let data = &img.channels[0];
                debug_assert!(!data.is_empty());
                parallel_sum(data) / data.len() as f32
            }
            PixelData::Rgb(img) => {
                let [r, g, b] = &img.channels;
                let total = parallel_sum(r) + parallel_sum(g) + parallel_sum(b);
                let count = r.len() + g.len() + b.len();
                total / count as f32
            }
        }
    }

    /// Save to file (PNG, JPEG, TIFF supported).
    pub fn save<P: AsRef<Path>>(&self, path: P) -> Result<(), ImageError> {
        let image: Image = self.into();
        image
            .save_file(path)
            .map_err(|source| ImageError::Save { source })
    }
}

impl StackableImage for AstroImage {
    fn dimensions(&self) -> ImageDimensions {
        self.dimensions()
    }

    fn channel(&self, c: usize) -> &[f32] {
        AstroImage::channel(self, c)
    }

    fn metadata(&self) -> &AstroImageMetadata {
        &self.metadata
    }

    fn load(path: &Path) -> Result<Self, ImageError> {
        AstroImage::from_file(path)
    }

    fn from_stacked(
        pixels: PixelData,
        metadata: AstroImageMetadata,
        dimensions: ImageDimensions,
    ) -> Self {
        AstroImage {
            metadata,
            dimensions,
            pixels,
        }
    }

    fn into_planes(self) -> arrayvec::ArrayVec<Buffer2<f32>, 3> {
        let mut planes = arrayvec::ArrayVec::new();
        match self.pixels {
            PixelData::L(img) => {
                let [buffer] = img.channels;
                planes.push(buffer);
            }
            PixelData::Rgb(img) => planes.extend(img.channels),
        }
        planes
    }
}

impl SubAssign<&AstroImage> for AstroImage {
    fn sub_assign(&mut self, rhs: &AstroImage) {
        assert_eq!(self.dimensions, rhs.dimensions, "Image dimensions mismatch");
        let w = self.dimensions.size.x;
        for c in 0..self.channels() {
            let dst = self.channel_mut(c).pixels_mut();
            let src = rhs.channel(c).pixels();
            dst.par_chunks_mut(w)
                .zip(src.par_chunks(w))
                .for_each(|(d_row, s_row)| {
                    for (d, s) in d_row.iter_mut().zip(s_row.iter()) {
                        *d -= s;
                    }
                });
        }
    }
}

impl From<&AstroImage> for Image {
    fn from(astro: &AstroImage) -> Self {
        // imaginarium owns the planar→interleaved transpose; each `PixelData`
        // arm already holds the `DeinterleavedImageData` it interleaves from
        // (borrowed, so an RGB master isn't cloned — `save` takes `&self`).
        match &astro.pixels {
            PixelData::L(planes) => Image::from(planes),
            PixelData::Rgb(planes) => Image::from(planes),
        }
    }
}

impl From<AstroImage> for Image {
    fn from(astro: AstroImage) -> Self {
        Image::from(&astro)
    }
}

/// The `f32` target format a given image deinterleaves into: `L_F32` for
/// grayscale, `RGB_F32` for color.
fn astro_target_format(image: &Image) -> ColorFormat {
    match image.desc.color_format.channel_count {
        ChannelCount::L => ColorFormat::L_F32,
        ChannelCount::Rgb | ChannelCount::Rgba => ColorFormat::RGB_F32,
    }
}

/// Deinterleave an already-`f32` (`L_F32` / `RGB_F32`) imaginarium image into a
/// planar [`AstroImage`]. This is the single unavoidable copy a per-channel op
/// pays to get planar data; callers must convert to f32 first.
fn astro_from_f32_image(image: &Image) -> AstroImage {
    let (width, height) = (image.desc.width, image.desc.height);
    // imaginarium owns the interleaved→planar transpose; the image is guaranteed
    // f32 here, so the variant deinterleaves into 1 or 3 planes.
    let pixels = match image.desc.color_format.channel_count {
        ChannelCount::L => PixelData::L(
            image
                .try_into()
                .expect("L_F32 image deinterleaves to 1 plane"),
        ),
        _ => PixelData::Rgb(
            image
                .try_into()
                .expect("RGB_F32 image deinterleaves to 3 planes"),
        ),
    };

    let dimensions = ImageDimensions::new((width, height), pixels.channels());
    AstroImage {
        metadata: AstroImageMetadata::default(),
        dimensions,
        pixels,
    }
}

impl From<Image> for AstroImage {
    fn from(image: Image) -> Self {
        let target = astro_target_format(&image);
        let image = image
            .convert(target)
            .expect("Failed to convert image to f32");
        astro_from_f32_image(&image)
    }
}

impl From<&Image> for AstroImage {
    fn from(image: &Image) -> Self {
        // Already f32: deinterleave straight from the borrow (one copy). A
        // non-f32 image is rare here (the processing path is RGB_F32) and needs
        // a format conversion first — `convert_to` reads the borrow directly.
        let target = astro_target_format(image);
        if image.desc.color_format == target {
            astro_from_f32_image(image)
        } else {
            let converted = image
                .convert_to(target)
                .expect("Failed to convert image to f32");
            astro_from_f32_image(&converted)
        }
    }
}

/// Deinterleave RGB data (RGBRGB...) into separate R, G, B planes.
fn deinterleave_rgb(interleaved: &[f32], r: &mut [f32], g: &mut [f32], b: &mut [f32]) {
    debug_assert_eq!(interleaved.len(), r.len() * 3);
    debug_assert_eq!(r.len(), g.len());
    debug_assert_eq!(g.len(), b.len());

    r.par_iter_mut()
        .zip(g.par_iter_mut())
        .zip(b.par_iter_mut())
        .enumerate()
        .for_each(|(i, ((r_val, g_val), b_val))| {
            let src_idx = i * 3;
            *r_val = interleaved[src_idx];
            *g_val = interleaved[src_idx + 1];
            *b_val = interleaved[src_idx + 2];
        });
}

/// Interleave separate R, G, B planes into RGB data (RGBRGB...). Test-only:
/// production interleaving goes through imaginarium's `Image` conversion; this
/// backs the `#[cfg(test)]` `into_interleaved_pixels` inspection helper.
#[cfg(test)]
fn interleave_rgb(r: &[f32], g: &[f32], b: &[f32], interleaved: &mut [f32]) {
    debug_assert_eq!(interleaved.len(), r.len() * 3);
    debug_assert_eq!(r.len(), g.len());
    debug_assert_eq!(g.len(), b.len());

    interleaved
        .par_chunks_mut(3)
        .enumerate()
        .for_each(|(i, rgb)| {
            rgb[0] = r[i];
            rgb[1] = g[i];
            rgb[2] = b[i];
        });
}

/// Per-pixel accessors used only by tests to inspect/construct planar buffers.
#[cfg(test)]
impl AstroImage {
    /// Get pixel value at (x, y) for grayscale images.
    pub fn get_pixel_gray(&self, x: usize, y: usize) -> f32 {
        debug_assert!(x < self.width() && y < self.height());
        debug_assert!(self.is_grayscale());
        self.channel(0)[y * self.width() + x]
    }

    /// Get mutable reference to pixel at (x, y) for grayscale images.
    pub fn get_pixel_gray_mut(&mut self, x: usize, y: usize) -> &mut f32 {
        debug_assert!(x < self.width() && y < self.height());
        debug_assert!(self.is_grayscale());
        let width = self.width();
        &mut self.channel_mut(0)[y * width + x]
    }

    /// Get pixel value at (x, y) for a specific channel.
    pub fn get_pixel_channel(&self, x: usize, y: usize, c: usize) -> f32 {
        debug_assert!(x < self.width() && y < self.height() && c < self.channels());
        self.channel(c)[y * self.width() + x]
    }

    /// Get RGB pixel values at (x, y).
    pub fn get_pixel_rgb(&self, x: usize, y: usize) -> Rgb {
        debug_assert!(x < self.width() && y < self.height());
        debug_assert!(self.is_rgb());
        let idx = y * self.width() + x;
        match &self.pixels {
            PixelData::Rgb(img) => {
                let [r, g, b] = &img.channels;
                Rgb {
                    r: r[idx],
                    g: g[idx],
                    b: b[idx],
                }
            }
            _ => unreachable!(),
        }
    }

    /// Set RGB pixel values at (x, y).
    pub fn set_pixel_rgb(&mut self, x: usize, y: usize, rgb: Rgb) {
        debug_assert!(x < self.width() && y < self.height());
        debug_assert!(self.is_rgb());
        let idx = y * self.width() + x;
        match &mut self.pixels {
            PixelData::Rgb(img) => {
                let [r, g, b] = &mut img.channels;
                r[idx] = rgb.r;
                g[idx] = rgb.g;
                b[idx] = rgb.b;
            }
            _ => unreachable!(),
        }
    }

    /// Consume and return interleaved pixels (RGBRGBRGB...).
    pub fn into_interleaved_pixels(self) -> Vec<f32> {
        match self.pixels {
            PixelData::L(img) => {
                let [data] = img.channels;
                data.into_vec()
            }
            PixelData::Rgb(img) => {
                let [r, g, b] = img.channels;
                let mut interleaved = vec![0.0f32; r.len() * 3];
                interleave_rgb(&r, &g, &b, &mut interleaved);
                interleaved
            }
        }
    }
}

#[cfg(test)]
mod tests;
