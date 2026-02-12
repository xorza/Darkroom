pub mod cfa;
mod fits;
pub(crate) mod sensor;

use anyhow::Result;
use rayon::prelude::*;

use imaginarium::{ChannelCount, ColorFormat, Image, ImageDesc};
use std::ops::SubAssign;
use std::path::Path;

use crate::common::Buffer2;

// ============================================================================
// BitPix - FITS pixel data types
// ============================================================================

/// FITS BITPIX values representing pixel data types.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum BitPix {
    #[default]
    UInt8,
    Int16,
    Int32,
    Int64,
    Float32,
    Float64,
}

impl BitPix {
    pub fn from_fits_value(value: i32) -> Self {
        match value {
            8 => BitPix::UInt8,
            16 => BitPix::Int16,
            32 => BitPix::Int32,
            64 => BitPix::Int64,
            -32 => BitPix::Float32,
            -64 => BitPix::Float64,
            _ => panic!("Unknown FITS BITPIX value: {value}"),
        }
    }

    pub fn to_fits_value(self) -> i32 {
        match self {
            BitPix::UInt8 => 8,
            BitPix::Int16 => 16,
            BitPix::Int32 => 32,
            BitPix::Int64 => 64,
            BitPix::Float32 => -32,
            BitPix::Float64 => -64,
        }
    }
}

// ============================================================================
// ImageDimensions
// ============================================================================

/// Image dimensions: width, height, and number of channels.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct ImageDimensions {
    pub width: usize,
    pub height: usize,
    pub channels: usize,
}

impl ImageDimensions {
    pub fn new(width: usize, height: usize, channels: usize) -> Self {
        assert!(width > 0, "Width must be positive");
        assert!(height > 0, "Height must be positive");
        assert!(
            channels == 1 || channels == 3,
            "Only 1 (grayscale) or 3 (RGB) channels supported, got {}",
            channels
        );
        Self {
            width,
            height,
            channels,
        }
    }

    pub fn pixel_count(&self) -> usize {
        self.width * self.height * self.channels
    }

    pub fn is_grayscale(&self) -> bool {
        self.channels == 1
    }

    pub fn is_rgb(&self) -> bool {
        self.channels == 3
    }
}

// ============================================================================
// AstroImageMetadata
// ============================================================================

/// Metadata extracted from FITS file headers.
#[derive(Debug, Clone, Default)]
pub struct AstroImageMetadata {
    pub object: Option<String>,
    pub instrument: Option<String>,
    pub telescope: Option<String>,
    pub date_obs: Option<String>,
    pub exposure_time: Option<f64>,
    pub iso: Option<u32>,
    pub bitpix: BitPix,
    pub header_dimensions: Vec<usize>,
    /// Whether the image has CFA pattern artifacts (from Bayer/X-Trans sensors).
    pub is_cfa: bool,
}

// ============================================================================
// PixelData
// ============================================================================

/// Pixel data storage - planar format for efficient per-channel operations.
#[derive(Debug, Clone)]
pub(crate) enum PixelData {
    L(Buffer2<f32>),
    Rgb([Buffer2<f32>; 3]),
}

// ============================================================================
// AstroImage
// ============================================================================

/// Represents an astronomical image.
#[derive(Debug, Clone)]
pub struct AstroImage {
    pub metadata: AstroImageMetadata,
    dimensions: ImageDimensions,
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
    pub fn from_file<P: AsRef<Path>>(path: P) -> Result<Self> {
        let path = path.as_ref();
        let ext = path
            .extension()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_lowercase();

        match ext.as_str() {
            "fit" | "fits" => fits::load_fits(path),
            "raf" | "cr2" | "cr3" | "nef" | "arw" | "dng" => crate::raw::load_raw(path),
            "tiff" | "tif" | "png" | "jpg" | "jpeg" => {
                let image = Image::read_file(path)?;
                Ok(image.into())
            }
            _ => anyhow::bail!("Unsupported file extension: {}", ext),
        }
    }

    /// Create from dimensions and interleaved pixel data (RGBRGBRGB...).
    pub fn from_pixels(dimensions: ImageDimensions, pixels: Vec<f32>) -> Self {
        assert_eq!(
            pixels.len(),
            dimensions.pixel_count(),
            "Pixel count mismatch: expected {}, got {}",
            dimensions.pixel_count(),
            pixels.len()
        );

        let width = dimensions.width;
        let height = dimensions.height;
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
        let expected_pixels_per_channel = dimensions.width * dimensions.height;
        let width = dimensions.width;
        let height = dimensions.height;

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
        self.dimensions.width
    }

    pub fn height(&self) -> usize {
        self.dimensions.height
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

    pub fn is_grayscale(&self) -> bool {
        matches!(self.pixels, PixelData::L(_))
    }

    pub fn is_rgb(&self) -> bool {
        matches!(self.pixels, PixelData::Rgb(_))
    }

    // ------------------------------------------------------------------------
    // Channel access
    // ------------------------------------------------------------------------

    /// Get channel as Buffer2 reference (0=L or R, 1=G, 2=B).
    pub fn channel(&self, c: usize) -> &Buffer2<f32> {
        match &self.pixels {
            PixelData::L(data) => {
                assert!(c == 0, "Grayscale image only has channel 0, got {}", c);
                data
            }
            PixelData::Rgb(channels) => {
                assert!(c < 3, "RGB image has channels 0-2, got {}", c);
                &channels[c]
            }
        }
    }

    /// Get channel as mutable Buffer2 reference.
    pub fn channel_mut(&mut self, c: usize) -> &mut Buffer2<f32> {
        match &mut self.pixels {
            PixelData::L(data) => {
                assert!(c == 0, "Grayscale image only has channel 0, got {}", c);
                data
            }
            PixelData::Rgb(channels) => {
                assert!(c < 3, "RGB image has channels 0-2, got {}", c);
                &mut channels[c]
            }
        }
    }

    // ------------------------------------------------------------------------
    // Pixel access
    // ------------------------------------------------------------------------

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
    pub fn get_pixel_rgb(&self, x: usize, y: usize) -> [f32; 3] {
        debug_assert!(x < self.width() && y < self.height());
        debug_assert!(self.is_rgb());
        let idx = y * self.width() + x;
        match &self.pixels {
            PixelData::Rgb([r, g, b]) => [r[idx], g[idx], b[idx]],
            _ => unreachable!(),
        }
    }

    /// Set RGB pixel values at (x, y).
    pub fn set_pixel_rgb(&mut self, x: usize, y: usize, rgb: [f32; 3]) {
        debug_assert!(x < self.width() && y < self.height());
        debug_assert!(self.is_rgb());
        let idx = y * self.width() + x;
        match &mut self.pixels {
            PixelData::Rgb([r, g, b]) => {
                r[idx] = rgb[0];
                g[idx] = rgb[1];
                b[idx] = rgb[2];
            }
            _ => unreachable!(),
        }
    }

    // ------------------------------------------------------------------------
    // Statistics
    // ------------------------------------------------------------------------

    /// Calculate mean pixel value across all channels.
    pub fn mean(&self) -> f32 {
        fn parallel_sum(values: &[f32]) -> f32 {
            values.par_chunks(8192).map(crate::math::sum_f32).sum()
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
    // Grayscale conversion
    // ------------------------------------------------------------------------

    /// Write grayscale data into an existing buffer (Rec. 709 luminance).
    pub fn write_grayscale_buffer(&self, output: &mut Buffer2<f32>) {
        debug_assert_eq!(output.width(), self.dimensions.width);
        debug_assert_eq!(output.height(), self.dimensions.height);

        match &self.pixels {
            PixelData::L(data) => output.pixels_mut().copy_from_slice(data),
            PixelData::Rgb([r, g, b]) => rgb_to_luminance(r, g, b, output.pixels_mut()),
        }
    }

    /// Convert to grayscale, consuming self (reuses R buffer for RGB).
    pub fn into_grayscale(self) -> Self {
        if self.is_grayscale() {
            return self;
        }

        let width = self.dimensions.width;
        let height = self.dimensions.height;

        match self.pixels {
            PixelData::Rgb([mut r, g, b]) => {
                rgb_to_luminance_inplace(&mut r, &g, &b);
                AstroImage {
                    metadata: self.metadata,
                    dimensions: ImageDimensions::new(width, height, 1),
                    pixels: PixelData::L(r),
                }
            }
            _ => unreachable!(),
        }
    }

    // ------------------------------------------------------------------------
    // Conversion / Export
    // ------------------------------------------------------------------------

    /// Save to file (PNG, JPEG, TIFF supported).
    pub fn save<P: AsRef<Path>>(&self, path: P) -> Result<()> {
        let image: Image = self.clone().into();
        image.save_file(path)?;
        Ok(())
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

// ============================================================================
// StackableImage implementation
// ============================================================================

impl crate::stacking::cache::StackableImage for AstroImage {
    fn dimensions(&self) -> ImageDimensions {
        self.dimensions()
    }

    fn channel(&self, c: usize) -> &[f32] {
        AstroImage::channel(self, c)
    }

    fn metadata(&self) -> &AstroImageMetadata {
        &self.metadata
    }

    fn load(path: &Path) -> Result<Self, crate::stacking::Error> {
        AstroImage::from_file(path).map_err(|e| crate::stacking::Error::ImageLoad {
            path: path.to_path_buf(),
            source: std::io::Error::other(e.to_string()),
        })
    }
}

// ============================================================================
// Arithmetic operators
// ============================================================================

impl SubAssign<&AstroImage> for AstroImage {
    fn sub_assign(&mut self, rhs: &AstroImage) {
        assert_eq!(self.dimensions, rhs.dimensions, "Image dimensions mismatch");
        let w = self.dimensions.width;
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

// ============================================================================
// From implementations
// ============================================================================

impl From<AstroImage> for Image {
    fn from(astro: AstroImage) -> Self {
        let width = astro.dimensions.width;
        let height = astro.dimensions.height;

        let (color_format, interleaved) = match astro.pixels {
            PixelData::L(data) => (ColorFormat::L_F32, data.into_vec()),
            PixelData::Rgb([r, g, b]) => {
                let mut interleaved = vec![0.0f32; r.len() * 3];
                interleave_rgb(&r, &g, &b, &mut interleaved);
                (ColorFormat::RGB_F32, interleaved)
            }
        };

        let desc = ImageDesc::new_with_stride(width, height, color_format);
        let bytes: Vec<u8> = bytemuck::cast_slice(&interleaved).to_vec();
        Image::new_with_data(desc, bytes).expect("Failed to create Image")
    }
}

impl From<Image> for AstroImage {
    fn from(image: Image) -> Self {
        let desc = image.desc();

        let target_format = match desc.color_format.channel_count {
            ChannelCount::L | ChannelCount::LA => ColorFormat::L_F32,
            ChannelCount::Rgb | ChannelCount::Rgba => ColorFormat::RGB_F32,
        };

        let image = image
            .convert(target_format)
            .expect("Failed to convert image to f32");

        let desc = image.desc();
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
            width,
            height,
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

// ============================================================================
// Private helper functions
// ============================================================================

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

/// Convert RGB planes to luminance using Rec. 709 weights.
fn rgb_to_luminance(r: &[f32], g: &[f32], b: &[f32], gray: &mut [f32]) {
    debug_assert_eq!(r.len(), g.len());
    debug_assert_eq!(g.len(), b.len());
    debug_assert_eq!(b.len(), gray.len());

    const R_WEIGHT: f32 = 0.2126;
    const G_WEIGHT: f32 = 0.7152;
    const B_WEIGHT: f32 = 0.0722;

    gray.par_iter_mut().enumerate().for_each(|(i, val)| {
        *val = R_WEIGHT * r[i] + G_WEIGHT * g[i] + B_WEIGHT * b[i];
    });
}

/// Convert RGB planes to luminance in-place, reusing the R buffer for output.
fn rgb_to_luminance_inplace(r: &mut Buffer2<f32>, g: &Buffer2<f32>, b: &Buffer2<f32>) {
    debug_assert_eq!(r.len(), g.len());
    debug_assert_eq!(g.len(), b.len());

    const R_WEIGHT: f32 = 0.2126;
    const G_WEIGHT: f32 = 0.7152;
    const B_WEIGHT: f32 = 0.0722;

    let g_slice = g.pixels();
    let b_slice = b.pixels();

    r.pixels_mut()
        .par_iter_mut()
        .enumerate()
        .for_each(|(i, val)| {
            *val = R_WEIGHT * *val + G_WEIGHT * g_slice[i] + B_WEIGHT * b_slice[i];
        });
}

#[cfg(test)]
mod tests;
