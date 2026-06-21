pub mod cfa;
pub mod error;
mod fits;
pub(crate) mod sensor;
#[cfg(test)]
mod synthetic_tests;

use rayon::prelude::*;

use error::ImageError;

use imaginarium::{ChannelCount, ChannelType, ColorFormat, Image, ImageDesc};
use std::ops::SubAssign;
use std::path::Path;

use common::Buffer2;
use common::Rgb;
use common::Vec2us;

use crate::io::raw::load_raw;
use crate::math::sum::sum_f32;
use crate::stacking::combine::cache::StackableImage;
use crate::stacking::combine::error::Error;

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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct ImageDimensions {
    /// Pixel dimensions as a `(width, height)` vector.
    pub size: Vec2us,
    pub channels: usize,
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
        Self { size, channels }
    }

    /// Total number of f32 samples: `width * height * channels`.
    /// For a 100x100 RGB image, returns 30000.
    pub fn sample_count(&self) -> usize {
        self.size.x * self.size.y * self.channels
    }

    /// Number of pixels: `width * height`.
    /// For a 100x100 RGB image, returns 10000.
    pub fn pixel_count(&self) -> usize {
        self.size.x * self.size.y
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
    L(Buffer2<f32>),
    Rgb([Buffer2<f32>; 3]),
}

impl PixelData {
    pub fn new_default(width: usize, height: usize, channels: usize) -> Self {
        match channels {
            1 => PixelData::L(Buffer2::new_default(width, height)),
            3 => PixelData::Rgb([
                Buffer2::new_default(width, height),
                Buffer2::new_default(width, height),
                Buffer2::new_default(width, height),
            ]),
            _ => panic!("Only 1 or 3 channels supported, got {channels}"),
        }
    }

    pub fn channel(&self, c: usize) -> &Buffer2<f32> {
        match self {
            PixelData::L(data) => {
                assert!(c == 0, "Grayscale image only has channel 0, got {c}");
                data
            }
            PixelData::Rgb(channels) => {
                assert!(c < 3, "RGB image has channels 0-2, got {c}");
                &channels[c]
            }
        }
    }

    pub fn channel_mut(&mut self, c: usize) -> &mut Buffer2<f32> {
        match self {
            PixelData::L(data) => {
                assert!(c == 0, "Grayscale image only has channel 0, got {c}");
                data
            }
            PixelData::Rgb(channels) => {
                assert!(c < 3, "RGB image has channels 0-2, got {c}");
                &mut channels[c]
            }
        }
    }

    pub fn channels(&self) -> usize {
        match self {
            PixelData::L(_) => 1,
            PixelData::Rgb(_) => 3,
        }
    }

    pub fn into_l(self) -> Buffer2<f32> {
        match self {
            PixelData::L(data) => data,
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
    // ------------------------------------------------------------------------
    // Constructors
    // ------------------------------------------------------------------------

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

        match ext.as_str() {
            "fit" | "fits" => fits::load_fits(path),
            "raf" | "cr2" | "cr3" | "nef" | "arw" | "dng" => load_raw(path),
            "tiff" | "tif" | "png" | "jpg" | "jpeg" => {
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
            }
            _ => Err(ImageError::UnsupportedFormat { extension: ext }),
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
            PixelData::L(Buffer2::new(width, height, pixels))
        } else {
            let mut r: Buffer2<f32> = Buffer2::new_default(width, height);
            let mut g: Buffer2<f32> = Buffer2::new_default(width, height);
            let mut b: Buffer2<f32> = Buffer2::new_default(width, height);
            deinterleave_rgb(&pixels, r.pixels_mut(), g.pixels_mut(), b.pixels_mut());
            PixelData::Rgb([r, g, b])
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
            PixelData::L(Buffer2::new(width, height, ch))
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
            PixelData::Rgb([
                Buffer2::new(width, height, r),
                Buffer2::new(width, height, g),
                Buffer2::new(width, height, b),
            ])
        };

        AstroImage {
            metadata: AstroImageMetadata::default(),
            dimensions,
            pixels: pixel_data,
        }
    }

    // ------------------------------------------------------------------------
    // Dimension accessors
    // ------------------------------------------------------------------------

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

    // ------------------------------------------------------------------------
    // Channel access
    // ------------------------------------------------------------------------

    /// Get channel as Buffer2 reference (0=L or R, 1=G, 2=B).
    pub fn channel(&self, c: usize) -> &Buffer2<f32> {
        self.pixels.channel(c)
    }

    /// Get channel as mutable Buffer2 reference.
    pub fn channel_mut(&mut self, c: usize) -> &mut Buffer2<f32> {
        self.pixels.channel_mut(c)
    }

    /// Transform every pixel in place, in parallel. The grayscale/RGB layout is dispatched
    /// **once**, then a homogeneous loop runs `mono` over each sample (grayscale) or `rgb` over
    /// each [`Rgb`] pixel — so per-pixel code never re-checks the channel count.
    pub(crate) fn par_map_pixels(
        &mut self,
        mono: impl Fn(f32) -> f32 + Sync,
        rgb: impl Fn(Rgb) -> Rgb + Sync,
    ) {
        match &mut self.pixels {
            PixelData::L(buf) => buf.pixels_mut().par_iter_mut().for_each(|v| *v = mono(*v)),
            PixelData::Rgb([r, g, b]) => {
                let (rp, gp, bp) = (r.pixels_mut(), g.pixels_mut(), b.pixels_mut());
                rp.par_iter_mut()
                    .zip(gp.par_iter_mut())
                    .zip(bp.par_iter_mut())
                    .for_each(|((rv, gv), bv)| {
                        let out = rgb(Rgb {
                            r: *rv,
                            g: *gv,
                            b: *bv,
                        });
                        *rv = out.r;
                        *gv = out.g;
                        *bv = out.b;
                    });
            }
        }
    }

    /// Per-pixel combined intensity as a 2D plane: the channel itself for grayscale, `(r+g+b)/3`
    /// for RGB. Keeps the image dimensions — a luminance plane, also usable as a sample set for
    /// image statistics.
    pub(crate) fn intensity_plane(&self) -> Buffer2<f32> {
        match &self.pixels {
            PixelData::L(buf) => buf.clone(),
            PixelData::Rgb([r, g, b]) => {
                let intensity = r
                    .pixels()
                    .iter()
                    .zip(g.pixels())
                    .zip(b.pixels())
                    .map(|((&r, &g), &b)| Rgb { r, g, b }.intensity())
                    .collect();
                Buffer2::new(self.width(), self.height(), intensity)
            }
        }
    }

    /// Remap pixel intensity from the `intensity` plane to the `mapped` plane (both precomputed over
    /// this image, matching its dimensions) in place. Hue-preserving: RGB channels are scaled by the
    /// gain `mapped/intensity`, with a max-cap so a brightened channel can't clip past white and
    /// shift hue; grayscale takes `mapped` directly. Output clamped to `[0, 1]`. Used by the
    /// display-domain enhancers (`local_contrast`, `hdr`).
    pub(crate) fn apply_intensity_remap(
        &mut self,
        intensity: &Buffer2<f32>,
        mapped: &Buffer2<f32>,
    ) {
        match &mut self.pixels {
            PixelData::L(buf) => buf
                .pixels_mut()
                .par_iter_mut()
                .zip(mapped.pixels().par_iter())
                .for_each(|(p, &m)| *p = m.clamp(0.0, 1.0)),
            PixelData::Rgb([r, g, b]) => {
                let (rp, gp, bp) = (r.pixels_mut(), g.pixels_mut(), b.pixels_mut());
                rp.par_iter_mut()
                    .zip(gp.par_iter_mut())
                    .zip(bp.par_iter_mut())
                    .zip(intensity.pixels().par_iter())
                    .zip(mapped.pixels().par_iter())
                    .for_each(|((((rv, gv), bv), &i), &m)| {
                        if i <= 0.0 {
                            return;
                        }
                        let gain = m / i;
                        let (mut nr, mut ng, mut nb) = (*rv * gain, *gv * gain, *bv * gain);
                        let maxc = nr.max(ng).max(nb);
                        if maxc > 1.0 {
                            let s = 1.0 / maxc;
                            nr *= s;
                            ng *= s;
                            nb *= s;
                        }
                        (*rv, *gv, *bv) = (nr.max(0.0), ng.max(0.0), nb.max(0.0));
                    });
            }
        }
    }

    // ------------------------------------------------------------------------
    // Statistics
    // ------------------------------------------------------------------------

    /// Calculate mean pixel value across all channels.
    pub fn mean(&self) -> f32 {
        fn parallel_sum(values: &[f32]) -> f32 {
            values.par_chunks(8192).map(sum_f32).sum()
        }

        match &self.pixels {
            PixelData::L(data) => {
                debug_assert!(!data.is_empty());
                parallel_sum(data) / data.len() as f32
            }
            PixelData::Rgb([r, g, b]) => {
                let total = parallel_sum(r) + parallel_sum(g) + parallel_sum(b);
                let count = r.len() + g.len() + b.len();
                total / count as f32
            }
        }
    }

    // ------------------------------------------------------------------------
    // Conversion / Export
    // ------------------------------------------------------------------------

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

    fn load(path: &Path) -> Result<Self, Error> {
        AstroImage::from_file(path).map_err(|e| Error::ImageLoad {
            path: path.to_path_buf(),
            source: std::io::Error::other(e),
        })
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
            PixelData::L(buffer) => planes.push(buffer),
            PixelData::Rgb(buffers) => planes.extend(buffers),
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
        let width = astro.dimensions.size.x;
        let height = astro.dimensions.size.y;

        // Build the byte buffer directly from the channel planes — one copy. (`save` takes `&self`,
        // so going through `&AstroImage` avoids cloning the whole image, which for an RGB master is
        // ~3× the largest allocation in the pipeline.)
        let (color_format, bytes): (ColorFormat, Vec<u8>) = match &astro.pixels {
            PixelData::L(data) => (
                ColorFormat::L_F32,
                bytemuck::cast_slice(data.pixels()).to_vec(),
            ),
            PixelData::Rgb([r, g, b]) => {
                let mut interleaved = vec![0.0f32; r.len() * 3];
                interleave_rgb(r, g, b, &mut interleaved);
                (
                    ColorFormat::RGB_F32,
                    bytemuck::cast_slice(&interleaved).to_vec(),
                )
            }
        };

        let desc = ImageDesc::new_with_stride(width, height, color_format);
        Image::new_with_data(desc, bytes).expect("Failed to create Image")
    }
}

impl From<AstroImage> for Image {
    fn from(astro: AstroImage) -> Self {
        Image::from(&astro)
    }
}

impl From<Image> for AstroImage {
    fn from(image: Image) -> Self {
        let desc = image.desc;

        let target_format = match desc.color_format.channel_count {
            ChannelCount::L | ChannelCount::LA => ColorFormat::L_F32,
            ChannelCount::Rgb | ChannelCount::Rgba => ColorFormat::RGB_F32,
        };

        let image = image
            .convert(target_format)
            .expect("Failed to convert image to f32");

        let desc = image.desc;
        let width = desc.width;
        let height = desc.height;
        let stride_f32 = desc.stride / std::mem::size_of::<f32>();
        let bytes = image.bytes();

        let pixel_data = if desc.color_format == ColorFormat::L_F32 {
            let mut data = vec![0.0f32; width * height];
            if desc.is_packed() {
                let pixels: &[f32] = bytemuck::cast_slice(bytes);
                data.copy_from_slice(pixels);
            } else {
                let row_bytes = width * std::mem::size_of::<f32>();
                let stride = desc.stride;
                data.par_chunks_mut(width).enumerate().for_each(|(y, row)| {
                    let src_offset = y * stride;
                    let src_row: &[f32] =
                        bytemuck::cast_slice(&bytes[src_offset..src_offset + row_bytes]);
                    row.copy_from_slice(src_row);
                });
            }
            PixelData::L(Buffer2::new(width, height, data))
        } else if desc.is_packed() {
            let pixels: &[f32] = bytemuck::cast_slice(bytes);
            let mut r: Buffer2<f32> = Buffer2::new_default(width, height);
            let mut g: Buffer2<f32> = Buffer2::new_default(width, height);
            let mut b: Buffer2<f32> = Buffer2::new_default(width, height);
            deinterleave_rgb(pixels, r.pixels_mut(), g.pixels_mut(), b.pixels_mut());
            PixelData::Rgb([r, g, b])
        } else {
            let pixels: &[f32] = bytemuck::cast_slice(bytes);
            let mut r: Buffer2<f32> = Buffer2::new_default(width, height);
            let mut g: Buffer2<f32> = Buffer2::new_default(width, height);
            let mut b: Buffer2<f32> = Buffer2::new_default(width, height);
            r.pixels_mut()
                .par_chunks_mut(width)
                .zip(g.pixels_mut().par_chunks_mut(width))
                .zip(b.pixels_mut().par_chunks_mut(width))
                .enumerate()
                .for_each(|(y, ((r_row, g_row), b_row))| {
                    let row_start = y * stride_f32;
                    for x in 0..width {
                        let src_idx = row_start + x * 3;
                        r_row[x] = pixels[src_idx];
                        g_row[x] = pixels[src_idx + 1];
                        b_row[x] = pixels[src_idx + 2];
                    }
                });
            PixelData::Rgb([r, g, b])
        };

        let dimensions = ImageDimensions::new(
            (width, height),
            if matches!(pixel_data, PixelData::L(_)) {
                1
            } else {
                3
            },
        );

        AstroImage {
            metadata: AstroImageMetadata::default(),
            dimensions,
            pixels: pixel_data,
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

/// Interleave separate R, G, B planes into RGB data (RGBRGB...).
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
            PixelData::Rgb([r, g, b]) => Rgb {
                r: r[idx],
                g: g[idx],
                b: b[idx],
            },
            _ => unreachable!(),
        }
    }

    /// Set RGB pixel values at (x, y).
    pub fn set_pixel_rgb(&mut self, x: usize, y: usize, rgb: Rgb) {
        debug_assert!(x < self.width() && y < self.height());
        debug_assert!(self.is_rgb());
        let idx = y * self.width() + x;
        match &mut self.pixels {
            PixelData::Rgb([r, g, b]) => {
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
            PixelData::L(data) => data.into_vec(),
            PixelData::Rgb([r, g, b]) => {
                let mut interleaved = vec![0.0f32; r.len() * 3];
                interleave_rgb(&r, &g, &b, &mut interleaved);
                interleaved
            }
        }
    }
}

#[cfg(test)]
mod tests;
