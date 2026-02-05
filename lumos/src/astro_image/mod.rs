pub(crate) mod demosaic;
mod fits;
pub(crate) mod hot_pixels;
pub(crate) mod libraw;
mod sensor;

pub use hot_pixels::HotPixelMap;

use anyhow::Result;
use rayon::prelude::*;

use imaginarium::{ChannelCount, ColorFormat, Image, ImageDesc};
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
            _ => BitPix::UInt8,
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
pub enum PixelData {
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
    pixels: PixelData,
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
            "raf" | "cr2" | "cr3" | "nef" | "arw" | "dng" => libraw::load_raw(path),
            "tiff" | "tif" | "png" | "jpg" | "jpeg" => {
                let image = Image::read_file(path)?;
                Ok(image.into())
            }
            _ => anyhow::bail!("Unsupported file extension: {}", ext),
        }
    }

    /// Load all astronomical images from a directory.
    pub fn load_from_directory<P: AsRef<Path>>(dir: P) -> Vec<Self> {
        common::file_utils::astro_image_files(dir.as_ref())
            .par_iter()
            .map(|path| Self::from_file(path).expect("Failed to load image"))
            .collect()
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
    // Per-channel operations
    // ------------------------------------------------------------------------

    /// Apply function to each channel with corresponding source channel.
    pub fn apply_from_channel<F>(&mut self, source: &AstroImage, f: F)
    where
        F: Fn(usize, &mut [f32], &[f32]) + Sync + Send,
    {
        assert_eq!(self.channels(), source.channels(), "Channel count mismatch");

        match (&mut self.pixels, &source.pixels) {
            (PixelData::L(dst), PixelData::L(src)) => f(0, dst, src),
            (PixelData::Rgb(dst_channels), PixelData::Rgb(src_channels)) => {
                dst_channels
                    .par_iter_mut()
                    .zip(src_channels.par_iter())
                    .enumerate()
                    .for_each(|(c, (dst, src))| f(c, dst, src));
            }
            _ => unreachable!("Channel count mismatch checked above"),
        }
    }

    /// Apply function to each channel in parallel.
    pub fn apply_per_channel_mut<F>(&mut self, f: F)
    where
        F: Fn(usize, &mut [f32]) + Sync + Send,
    {
        match &mut self.pixels {
            PixelData::L(data) => f(0, data),
            PixelData::Rgb(channels) => {
                channels
                    .par_iter_mut()
                    .enumerate()
                    .for_each(|(c, data)| f(c, data));
            }
        }
    }

    // ------------------------------------------------------------------------
    // Statistics
    // ------------------------------------------------------------------------

    /// Calculate mean pixel value across all channels.
    pub fn mean(&self) -> f32 {
        fn parallel_sum(values: &[f32]) -> f32 {
            common::parallel::par_iter_auto(values.len())
                .map(|(_, start, end)| crate::math::sum_f32(&values[start..end]))
                .sum()
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

    /// Compute grayscale as a new buffer (Rec. 709 luminance).
    pub fn to_grayscale_buffer(&self) -> Buffer2<f32> {
        match &self.pixels {
            PixelData::L(data) => data.clone(),
            PixelData::Rgb([r, g, b]) => {
                let mut gray = vec![0.0f32; r.len()];
                rgb_to_luminance(r, g, b, &mut gray);
                Buffer2::new(self.dimensions.width, self.dimensions.height, gray)
            }
        }
    }

    /// Convert to grayscale into an existing buffer.
    pub fn into_grayscale_buffer(&self, output: &mut Buffer2<f32>) {
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

    /// Convert to imaginarium::Image.
    pub fn into_image(self) -> Image {
        self.into()
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

    /// Consume and return planar pixel data.
    pub fn into_pixels(self) -> PixelData {
        self.pixels
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
                common::parallel::par_chunks_auto_aligned(&mut data, width).for_each(
                    |(start_row, chunk)| {
                        let rows_in_chunk = chunk.len() / width;
                        for row_offset in 0..rows_in_chunk {
                            let y = start_row + row_offset;
                            let src_offset = y * stride;
                            let src_row: &[f32] =
                                bytemuck::cast_slice(&bytes[src_offset..src_offset + row_bytes]);
                            let dst_start = row_offset * width;
                            chunk[dst_start..dst_start + width].copy_from_slice(src_row);
                        }
                    },
                );
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
            common::parallel::par_chunks_auto_aligned_zip3(
                r.pixels_mut(),
                g.pixels_mut(),
                b.pixels_mut(),
                width,
            )
            .for_each(|(start_row, (r_row, g_row, b_row))| {
                let rows_in_chunk = r_row.len() / width;
                for row_offset in 0..rows_in_chunk {
                    let y = start_row + row_offset;
                    let row_start = y * stride_f32;
                    let dst_start = row_offset * width;
                    for x in 0..width {
                        let src_idx = row_start + x * 3;
                        r_row[dst_start + x] = pixels[src_idx];
                        g_row[dst_start + x] = pixels[src_idx + 1];
                        b_row[dst_start + x] = pixels[src_idx + 2];
                    }
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
    use common::parallel::par_chunks_auto_aligned_zip3;

    debug_assert_eq!(interleaved.len(), r.len() * 3);
    debug_assert_eq!(r.len(), g.len());
    debug_assert_eq!(g.len(), b.len());

    // width=1 since we're treating it as a flat buffer
    par_chunks_auto_aligned_zip3(r, g, b, 1).for_each(|(start, (r_chunk, g_chunk, b_chunk))| {
        for i in 0..r_chunk.len() {
            let src_idx = (start + i) * 3;
            r_chunk[i] = interleaved[src_idx];
            g_chunk[i] = interleaved[src_idx + 1];
            b_chunk[i] = interleaved[src_idx + 2];
        }
    });
}

/// Interleave separate R, G, B planes into RGB data (RGBRGB...).
fn interleave_rgb(r: &[f32], g: &[f32], b: &[f32], interleaved: &mut [f32]) {
    debug_assert_eq!(interleaved.len(), r.len() * 3);
    debug_assert_eq!(r.len(), g.len());
    debug_assert_eq!(g.len(), b.len());

    common::parallel::par_chunks_auto(interleaved).for_each(|(start_idx, chunk)| {
        for (j, val) in chunk.iter_mut().enumerate() {
            let i = start_idx + j;
            let pixel_idx = i / 3;
            let channel = i % 3;
            *val = match channel {
                0 => r[pixel_idx],
                1 => g[pixel_idx],
                _ => b[pixel_idx],
            };
        }
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

    common::parallel::par_chunks_auto(gray).for_each(|(start_idx, chunk)| {
        for (j, val) in chunk.iter_mut().enumerate() {
            let i = start_idx + j;
            *val = R_WEIGHT * r[i] + G_WEIGHT * g[i] + B_WEIGHT * b[i];
        }
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

    common::parallel::par_chunks_auto(r.pixels_mut()).for_each(|(start_idx, chunk)| {
        for (j, val) in chunk.iter_mut().enumerate() {
            let i = start_idx + j;
            *val = R_WEIGHT * *val + G_WEIGHT * g_slice[i] + B_WEIGHT * b_slice[i];
        }
    });
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use common::test_utils::test_output_path;

    use crate::testing::{calibration_dir, init_tracing};

    use super::*;

    #[test]
    fn test_metadata_default() {
        let meta = AstroImageMetadata::default();
        assert!(meta.object.is_none());
        assert!(meta.header_dimensions.is_empty());
    }

    #[test]
    fn test_convert_to_imaginarium_image_grayscale() {
        let astro = AstroImage::from_pixels(
            ImageDimensions::new(3, 2, 1),
            vec![0.0, 0.25, 0.5, 0.75, 1.0, 0.5],
        );

        let image: Image = astro.into();
        let desc = image.desc();

        assert_eq!(desc.width, 3);
        assert_eq!(desc.height, 2);
        assert_eq!(desc.color_format, ColorFormat::L_F32);

        let pixels: &[f32] = bytemuck::cast_slice(image.bytes());
        assert_eq!(pixels.len(), 6);
        assert_eq!(pixels[0], 0.0);
        assert_eq!(pixels[1], 0.25);
        assert_eq!(pixels[4], 1.0);
    }

    #[test]
    fn test_convert_to_imaginarium_image_rgb() {
        let astro = AstroImage::from_pixels(
            ImageDimensions::new(2, 2, 3),
            vec![
                1.0, 0.0, 0.0, // red
                0.0, 1.0, 0.0, // green
                0.0, 0.0, 1.0, // blue
                1.0, 1.0, 1.0, // white
            ],
        );

        let image: Image = astro.into();
        let desc = image.desc();

        assert_eq!(desc.width, 2);
        assert_eq!(desc.height, 2);
        assert_eq!(desc.color_format, ColorFormat::RGB_F32);

        let pixels: &[f32] = bytemuck::cast_slice(image.bytes());
        assert_eq!(pixels.len(), 12);
        assert_eq!(pixels[0], 1.0);
        assert_eq!(pixels[1], 0.0);
        assert_eq!(pixels[2], 0.0);
        assert_eq!(pixels[9], 1.0);
        assert_eq!(pixels[10], 1.0);
        assert_eq!(pixels[11], 1.0);
    }

    #[test]
    fn test_convert_fits_to_imaginarium_image() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../test_resources/full_example.fits"
        );
        let astro = AstroImage::from_file(path).unwrap();
        let image: Image = astro.into();

        let desc = image.desc();
        assert_eq!(desc.width, 100);
        assert_eq!(desc.height, 100);
        assert_eq!(desc.color_format, ColorFormat::L_F32);
    }

    #[test]
    fn test_load_full_example_fits() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../test_resources/full_example.fits"
        );
        let image = AstroImage::from_file(path).unwrap();

        assert_eq!(image.width(), 100);
        assert_eq!(image.height(), 100);
        assert_eq!(image.channels(), 1);
        assert!(image.is_grayscale());
        assert_eq!(image.pixel_count(), 10000);
        assert_eq!(image.metadata.bitpix, BitPix::Int32);
        assert_eq!(image.metadata.header_dimensions, vec![100, 100]);

        let pixel = image.get_pixel_gray(5, 20);
        assert_eq!(pixel, 152.0);
    }

    #[test]
    fn test_from_image_no_stride_padding() {
        let desc = ImageDesc::new_with_stride(3, 2, ColorFormat::L_F32);
        let pixels: Vec<f32> = vec![1.0, 2.0, 3.0, 4.0, 5.0, 6.0];
        let bytes: Vec<u8> = bytemuck::cast_slice(&pixels).to_vec();
        let image = Image::new_with_data(desc, bytes).unwrap();

        let astro: AstroImage = image.into();

        assert_eq!(astro.width(), 3);
        assert_eq!(astro.height(), 2);
        assert_eq!(astro.channels(), 1);
        assert_eq!(astro.channel(0).pixels(), &pixels[..]);
    }

    #[test]
    fn test_mean() {
        let image =
            AstroImage::from_pixels(ImageDimensions::new(2, 2, 1), vec![1.0, 2.0, 3.0, 4.0]);
        assert!((image.mean() - 2.5).abs() < f32::EPSILON);
    }

    #[test]
    fn test_save_grayscale_tiff() {
        let image =
            AstroImage::from_pixels(ImageDimensions::new(2, 2, 1), vec![0.1, 0.2, 0.3, 0.4]);
        let output_path = test_output_path("astro_save_gray.tiff");

        image.save(&output_path).unwrap();
        assert!(output_path.exists());

        let loaded = AstroImage::from_file(&output_path).unwrap();
        assert_eq!(loaded.width(), 2);
        assert_eq!(loaded.height(), 2);
        assert_eq!(loaded.channels(), 1);
    }

    #[test]
    fn test_save_rgb_tiff() {
        let image = AstroImage::from_pixels(
            ImageDimensions::new(2, 2, 3),
            vec![1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 1.0, 1.0, 1.0],
        );
        let output_path = test_output_path("astro_save_rgb.tiff");

        image.save(&output_path).unwrap();
        assert!(output_path.exists());

        let loaded = AstroImage::from_file(&output_path).unwrap();
        assert_eq!(loaded.width(), 2);
        assert_eq!(loaded.height(), 2);
        assert_eq!(loaded.channels(), 3);
    }

    #[test]
    fn test_save_invalid_extension() {
        let image =
            AstroImage::from_pixels(ImageDimensions::new(2, 2, 1), vec![0.1, 0.2, 0.3, 0.4]);
        let output_path = test_output_path("astro_save_invalid.xyz");

        let result = image.save(&output_path);
        assert!(result.is_err());
    }

    #[test]
    fn test_calibrate_bias_subtraction() {
        use crate::{CalibrationMasters, StackingMethod};

        let mut light = AstroImage::from_pixels(
            ImageDimensions::new(2, 2, 1),
            vec![100.0, 200.0, 150.0, 250.0],
        );
        let bias = AstroImage::from_pixels(ImageDimensions::new(2, 2, 1), vec![5.0, 5.0, 5.0, 5.0]);

        let masters = CalibrationMasters {
            master_dark: None,
            master_flat: None,
            master_bias: Some(bias),
            hot_pixel_map: None,
            method: StackingMethod::default(),
        };

        masters.calibrate(&mut light);
        assert_eq!(light.channel(0).pixels(), &[95.0, 195.0, 145.0, 245.0]);
    }

    #[test]
    fn test_calibrate_dark_subtraction() {
        use crate::{CalibrationMasters, StackingMethod};

        let mut light = AstroImage::from_pixels(
            ImageDimensions::new(2, 2, 1),
            vec![100.0, 200.0, 150.0, 250.0],
        );
        let dark =
            AstroImage::from_pixels(ImageDimensions::new(2, 2, 1), vec![10.0, 20.0, 15.0, 25.0]);

        let masters = CalibrationMasters {
            master_dark: Some(dark),
            master_flat: None,
            master_bias: None,
            hot_pixel_map: None,
            method: StackingMethod::default(),
        };

        masters.calibrate(&mut light);
        assert_eq!(light.channel(0).pixels(), &[90.0, 180.0, 135.0, 225.0]);
    }

    #[test]
    fn test_calibrate_flat_correction() {
        use crate::{CalibrationMasters, StackingMethod};

        let mut light = AstroImage::from_pixels(
            ImageDimensions::new(2, 2, 1),
            vec![100.0, 200.0, 150.0, 250.0],
        );
        let flat = AstroImage::from_pixels(ImageDimensions::new(2, 2, 1), vec![0.8, 1.0, 1.2, 1.0]);

        let masters = CalibrationMasters {
            master_dark: None,
            master_flat: Some(flat),
            master_bias: None,
            hot_pixel_map: None,
            method: StackingMethod::default(),
        };

        masters.calibrate(&mut light);

        assert!((light.channel(0)[0] - 125.0).abs() < 0.01);
        assert!((light.channel(0)[1] - 200.0).abs() < 0.01);
        assert!((light.channel(0)[2] - 125.0).abs() < 0.01);
        assert!((light.channel(0)[3] - 250.0).abs() < 0.01);
    }

    #[test]
    fn test_calibrate_full() {
        use crate::{CalibrationMasters, StackingMethod};

        let mut light = AstroImage::from_pixels(
            ImageDimensions::new(2, 2, 1),
            vec![115.0, 225.0, 170.0, 280.0],
        );
        let bias = AstroImage::from_pixels(ImageDimensions::new(2, 2, 1), vec![5.0, 5.0, 5.0, 5.0]);
        let dark =
            AstroImage::from_pixels(ImageDimensions::new(2, 2, 1), vec![10.0, 20.0, 15.0, 25.0]);
        let flat = AstroImage::from_pixels(ImageDimensions::new(2, 2, 1), vec![0.8, 1.0, 1.2, 1.0]);

        let masters = CalibrationMasters {
            master_dark: Some(dark),
            master_flat: Some(flat),
            master_bias: Some(bias),
            hot_pixel_map: None,
            method: StackingMethod::default(),
        };

        masters.calibrate(&mut light);

        assert!((light.channel(0)[0] - 125.0).abs() < 0.01);
        assert!((light.channel(0)[1] - 200.0).abs() < 0.01);
        assert!((light.channel(0)[2] - 125.0).abs() < 0.01);
        assert!((light.channel(0)[3] - 250.0).abs() < 0.01);
    }

    #[test]
    fn test_roundtrip_astro_to_image_to_astro() {
        let gray = AstroImage::from_pixels(
            ImageDimensions::new(3, 2, 1),
            vec![0.1, 0.2, 0.3, 0.4, 0.5, 0.6],
        );

        let image: Image = gray.clone().into();
        let restored: AstroImage = image.into();

        assert_eq!(restored.dimensions(), gray.dimensions());
        for (a, b) in gray.channel(0).iter().zip(restored.channel(0).iter()) {
            assert!((a - b).abs() < 1e-6);
        }

        let rgb = AstroImage::from_pixels(
            ImageDimensions::new(2, 2, 3),
            vec![1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.5, 0.5, 0.5],
        );

        let image: Image = rgb.clone().into();
        let restored: AstroImage = image.into();

        assert_eq!(restored.dimensions(), rgb.dimensions());
        for c in 0..rgb.channels() {
            for (a, b) in rgb.channel(c).iter().zip(restored.channel(c).iter()) {
                assert!((a - b).abs() < 1e-6);
            }
        }
    }

    #[test]
    fn test_image_rgba_to_astro_drops_alpha() {
        let desc = ImageDesc::new_with_stride(2, 1, ColorFormat::RGBA_F32);
        let pixels: Vec<f32> = vec![1.0, 0.0, 0.0, 0.5, 0.0, 1.0, 0.0, 1.0];
        let bytes: Vec<u8> = bytemuck::cast_slice(&pixels).to_vec();
        let image = Image::new_with_data(desc, bytes).unwrap();

        let astro: AstroImage = image.into();

        assert_eq!(astro.channels(), 3);
        assert!((astro.channel(0)[0] - 1.0).abs() < 1e-6);
        assert!((astro.channel(1)[0] - 0.0).abs() < 1e-6);
        assert!((astro.channel(2)[0] - 0.0).abs() < 1e-6);
        assert!((astro.channel(0)[1] - 0.0).abs() < 1e-6);
        assert!((astro.channel(1)[1] - 1.0).abs() < 1e-6);
        assert!((astro.channel(2)[1] - 0.0).abs() < 1e-6);
    }

    #[test]
    fn test_image_gray_alpha_to_astro_drops_alpha() {
        let desc = ImageDesc::new_with_stride(2, 1, ColorFormat::LA_F32);
        let pixels: Vec<f32> = vec![0.5, 0.8, 0.9, 1.0];
        let bytes: Vec<u8> = bytemuck::cast_slice(&pixels).to_vec();
        let image = Image::new_with_data(desc, bytes).unwrap();

        let astro: AstroImage = image.into();

        assert_eq!(astro.channels(), 1);
        assert!((astro.channel(0)[0] - 0.5).abs() < 1e-6);
        assert!((astro.channel(0)[1] - 0.9).abs() < 1e-6);
    }

    #[test]
    #[cfg_attr(not(feature = "real-data"), ignore)]
    fn test_load_single_raw_from_env() {
        init_tracing();

        let Some(cal_dir) = calibration_dir() else {
            eprintln!("LUMOS_CALIBRATION_DIR not set, skipping test");
            return;
        };

        let lights_dir = cal_dir.join("Lights");
        if !lights_dir.exists() {
            eprintln!("Lights directory not found, skipping test");
            return;
        }

        let files = common::file_utils::astro_image_files(&lights_dir);
        let Some(first_file) = files.first() else {
            eprintln!("No image files in Lights, skipping test");
            return;
        };

        println!("Loading file: {:?}", first_file);

        let image = AstroImage::from_file(first_file).expect("Failed to load image");

        println!(
            "Loaded image: {}x{}x{}",
            image.width(),
            image.height(),
            image.channels()
        );
        println!("Mean: {}", image.mean());

        assert!(image.width() > 0);
        assert!(image.height() > 0);
        assert_eq!(image.channels(), 3);

        let image: imaginarium::Image = image.into();
        image
            .save_file(test_output_path("light_from_raw.tiff"))
            .unwrap();
    }

    #[test]
    #[cfg_attr(not(feature = "real-data"), ignore)]
    fn test_calibrate_light_from_env() {
        use crate::testing::{calibration_dir, calibration_masters_dir, init_tracing};
        use crate::{CalibrationMasters, StackingMethod};

        init_tracing();

        let Some(cal_dir) = calibration_dir() else {
            eprintln!("LUMOS_CALIBRATION_DIR not set, skipping test");
            return;
        };

        let lights_dir = cal_dir.join("Lights");
        if !lights_dir.exists() {
            eprintln!("Lights directory not found, skipping test");
            return;
        }

        let files = common::file_utils::astro_image_files(&lights_dir);
        let Some(first_file) = files.first() else {
            eprintln!("No image files in Lights, skipping test");
            return;
        };

        let start = std::time::Instant::now();
        println!("Loading light frame: {:?}", first_file);
        let mut light = AstroImage::from_file(first_file).expect("Failed to load light frame");
        println!("  Load light: {:?}", start.elapsed());

        let original_dimensions = light.dimensions();

        println!(
            "Loaded light frame: {}x{}x{}",
            light.width(),
            light.height(),
            light.channels()
        );

        let Some(masters_dir) = calibration_masters_dir() else {
            eprintln!("calibration_masters directory not found, skipping test");
            return;
        };

        let start = std::time::Instant::now();
        let masters = CalibrationMasters::load(&masters_dir, StackingMethod::default()).unwrap();
        println!("  Load masters: {:?}", start.elapsed());

        println!(
            "Loaded masters: dark={}, flat={}, bias={}",
            masters.master_dark.is_some(),
            masters.master_flat.is_some(),
            masters.master_bias.is_some()
        );

        if let Some(ref hot_map) = masters.hot_pixel_map {
            println!(
                "  Hot pixels: {} ({:.4}%)",
                hot_map.count,
                hot_map.percentage()
            );
        }

        let start = std::time::Instant::now();
        masters.calibrate(&mut light);
        println!("  Calibrate: {:?}", start.elapsed());

        println!(
            "Calibrated frame: {}x{}x{}",
            light.width(),
            light.height(),
            light.channels()
        );
        println!("Mean: {}", light.mean());

        assert_eq!(light.dimensions(), original_dimensions);

        let start = std::time::Instant::now();
        let output_path = common::test_utils::test_output_path("calibrated_light.tiff");
        let img: imaginarium::Image = light.into();
        img.save_file(&output_path).unwrap();
        println!("  Save: {:?}", start.elapsed());

        println!("Saved calibrated image to: {:?}", output_path);
        assert!(output_path.exists());
    }

    #[test]
    fn test_rgb_image_creation_and_operations() {
        let image = AstroImage::from_pixels(
            ImageDimensions::new(2, 2, 3),
            vec![
                10.0, 20.0, 30.0, 40.0, 50.0, 60.0, 70.0, 80.0, 90.0, 100.0, 110.0, 120.0,
            ],
        );

        assert_eq!(image.width(), 2);
        assert_eq!(image.height(), 2);
        assert_eq!(image.channels(), 3);
        assert!(!image.is_grayscale());
        assert_eq!(image.pixel_count(), 12);

        let expected_mean: f32 =
            (10.0 + 20.0 + 30.0 + 40.0 + 50.0 + 60.0 + 70.0 + 80.0 + 90.0 + 100.0 + 110.0 + 120.0)
                / 12.0;
        assert!((image.mean() - expected_mean).abs() < f32::EPSILON);
    }

    #[test]
    fn test_calibrate_rgb_with_rgb_masters() {
        use crate::{CalibrationMasters, StackingMethod};

        let mut light = AstroImage::from_pixels(
            ImageDimensions::new(2, 2, 3),
            vec![
                100.0, 100.0, 100.0, 200.0, 200.0, 200.0, 150.0, 150.0, 150.0, 250.0, 250.0, 250.0,
            ],
        );

        let bias = AstroImage::from_pixels(ImageDimensions::new(2, 2, 3), vec![5.0; 12]);

        let masters = CalibrationMasters {
            master_dark: None,
            master_flat: None,
            master_bias: Some(bias),
            hot_pixel_map: None,
            method: StackingMethod::default(),
        };

        masters.calibrate(&mut light);

        assert_eq!(light.channel(0)[0], 95.0);
        assert_eq!(light.channel(1)[0], 95.0);
        assert_eq!(light.channel(2)[0], 95.0);
        assert_eq!(light.channel(0)[1], 195.0);
    }

    #[test]
    #[should_panic(expected = "don't match")]
    fn test_calibrate_rgb_with_grayscale_bias_panics() {
        use crate::{CalibrationMasters, StackingMethod};

        let mut light = AstroImage::from_pixels(ImageDimensions::new(2, 2, 3), vec![100.0; 12]);
        let bias = AstroImage::from_pixels(ImageDimensions::new(2, 2, 1), vec![5.0; 4]);

        let masters = CalibrationMasters {
            master_dark: None,
            master_flat: None,
            master_bias: Some(bias),
            hot_pixel_map: None,
            method: StackingMethod::default(),
        };

        masters.calibrate(&mut light);
    }

    #[test]
    #[should_panic(expected = "don't match")]
    fn test_calibrate_grayscale_with_rgb_dark_panics() {
        use crate::{CalibrationMasters, StackingMethod};

        let mut light = AstroImage::from_pixels(ImageDimensions::new(2, 2, 1), vec![100.0; 4]);
        let dark = AstroImage::from_pixels(ImageDimensions::new(2, 2, 3), vec![10.0; 12]);

        let masters = CalibrationMasters {
            master_dark: Some(dark),
            master_flat: None,
            master_bias: None,
            hot_pixel_map: None,
            method: StackingMethod::default(),
        };

        masters.calibrate(&mut light);
    }

    #[test]
    fn test_get_pixel_gray() {
        let image = AstroImage::from_pixels(
            ImageDimensions::new(3, 2, 1),
            vec![1.0, 2.0, 3.0, 4.0, 5.0, 6.0],
        );

        assert_eq!(image.get_pixel_gray(0, 0), 1.0);
        assert_eq!(image.get_pixel_gray(2, 0), 3.0);
        assert_eq!(image.get_pixel_gray(0, 1), 4.0);
        assert_eq!(image.get_pixel_gray(2, 1), 6.0);
        assert_eq!(image.get_pixel_channel(1, 0, 0), 2.0);
        assert_eq!(image.get_pixel_channel(1, 1, 0), 5.0);
    }

    #[test]
    fn test_get_pixel_channel_rgb() {
        let image = AstroImage::from_pixels(
            ImageDimensions::new(2, 2, 3),
            vec![
                1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0, 9.0, 10.0, 11.0, 12.0,
            ],
        );

        assert_eq!(image.get_pixel_channel(0, 0, 0), 1.0);
        assert_eq!(image.get_pixel_channel(0, 0, 1), 2.0);
        assert_eq!(image.get_pixel_channel(0, 0, 2), 3.0);
        assert_eq!(image.get_pixel_channel(1, 0, 0), 4.0);
        assert_eq!(image.get_pixel_channel(1, 0, 1), 5.0);
        assert_eq!(image.get_pixel_channel(1, 0, 2), 6.0);
        assert_eq!(image.get_pixel_channel(0, 1, 0), 7.0);
        assert_eq!(image.get_pixel_channel(0, 1, 1), 8.0);
        assert_eq!(image.get_pixel_channel(0, 1, 2), 9.0);
        assert_eq!(image.get_pixel_channel(1, 1, 0), 10.0);
        assert_eq!(image.get_pixel_channel(1, 1, 1), 11.0);
        assert_eq!(image.get_pixel_channel(1, 1, 2), 12.0);
    }

    #[test]
    fn test_to_grayscale_rgb() {
        let rgb = AstroImage::from_pixels(
            ImageDimensions::new(2, 1, 3),
            vec![1.0, 0.0, 0.0, 0.0, 1.0, 0.0],
        );

        let gray = rgb.into_grayscale();

        assert!(gray.is_grayscale());
        assert_eq!(gray.channels(), 1);
        assert!((gray.channel(0)[0] - 0.2126).abs() < 1e-4);
        assert!((gray.channel(0)[1] - 0.7152).abs() < 1e-4);
    }

    #[test]
    fn test_to_grayscale_already_gray() {
        let gray = AstroImage::from_pixels(ImageDimensions::new(2, 2, 1), vec![1.0, 2.0, 3.0, 4.0]);
        let result = gray.into_grayscale();

        assert!(result.is_grayscale());
        assert_eq!(result.channel(0).pixels(), &[1.0, 2.0, 3.0, 4.0]);
    }

    #[test]
    fn test_from_planar_channels_grayscale() {
        let channels = vec![vec![1.0, 2.0, 3.0, 4.0]];
        let image = AstroImage::from_planar_channels(ImageDimensions::new(2, 2, 1), channels);

        assert!(image.is_grayscale());
        assert_eq!(image.channel(0).pixels(), &[1.0, 2.0, 3.0, 4.0]);
    }

    #[test]
    fn test_from_planar_channels_rgb() {
        let image = AstroImage::from_planar_channels(
            ImageDimensions::new(2, 1, 3),
            vec![vec![1.0, 2.0], vec![3.0, 4.0], vec![5.0, 6.0]],
        );

        assert!(image.is_rgb());
        assert_eq!(image.channel(0).pixels(), &[1.0, 2.0]);
        assert_eq!(image.channel(1).pixels(), &[3.0, 4.0]);
        assert_eq!(image.channel(2).pixels(), &[5.0, 6.0]);
    }

    #[test]
    fn test_channel_mut() {
        let mut image =
            AstroImage::from_pixels(ImageDimensions::new(2, 2, 1), vec![1.0, 2.0, 3.0, 4.0]);

        image.channel_mut(0)[0] = 10.0;
        image.channel_mut(0)[3] = 40.0;

        assert_eq!(image.channel(0).pixels(), &[10.0, 2.0, 3.0, 40.0]);
    }

    #[test]
    fn test_get_pixel_rgb() {
        let image = AstroImage::from_pixels(
            ImageDimensions::new(2, 1, 3),
            vec![1.0, 2.0, 3.0, 4.0, 5.0, 6.0],
        );

        assert_eq!(image.get_pixel_rgb(0, 0), [1.0, 2.0, 3.0]);
        assert_eq!(image.get_pixel_rgb(1, 0), [4.0, 5.0, 6.0]);
    }

    #[test]
    fn test_set_pixel_rgb() {
        let mut image = AstroImage::from_pixels(
            ImageDimensions::new(2, 1, 3),
            vec![0.0, 0.0, 0.0, 0.0, 0.0, 0.0],
        );

        image.set_pixel_rgb(0, 0, [1.0, 2.0, 3.0]);
        image.set_pixel_rgb(1, 0, [4.0, 5.0, 6.0]);

        assert_eq!(image.get_pixel_rgb(0, 0), [1.0, 2.0, 3.0]);
        assert_eq!(image.get_pixel_rgb(1, 0), [4.0, 5.0, 6.0]);
    }

    #[test]
    fn test_get_pixel_gray_mut() {
        let mut image =
            AstroImage::from_pixels(ImageDimensions::new(2, 2, 1), vec![1.0, 2.0, 3.0, 4.0]);

        *image.get_pixel_gray_mut(0, 0) = 10.0;
        *image.get_pixel_gray_mut(1, 1) = 40.0;

        assert_eq!(image.get_pixel_gray(0, 0), 10.0);
        assert_eq!(image.get_pixel_gray(1, 1), 40.0);
    }

    #[test]
    fn test_into_interleaved_pixels_grayscale() {
        let image =
            AstroImage::from_pixels(ImageDimensions::new(2, 2, 1), vec![1.0, 2.0, 3.0, 4.0]);

        let interleaved = image.into_interleaved_pixels();
        assert_eq!(interleaved, vec![1.0, 2.0, 3.0, 4.0]);
    }

    #[test]
    fn test_into_interleaved_pixels_rgb() {
        let image = AstroImage::from_planar_channels(
            ImageDimensions::new(2, 1, 3),
            vec![vec![1.0, 4.0], vec![2.0, 5.0], vec![3.0, 6.0]],
        );

        let interleaved = image.into_interleaved_pixels();
        assert_eq!(interleaved, vec![1.0, 2.0, 3.0, 4.0, 5.0, 6.0]);
    }

    #[test]
    fn test_apply_from_channel() {
        let mut image = AstroImage::from_pixels(
            ImageDimensions::new(2, 1, 3),
            vec![1.0, 2.0, 3.0, 4.0, 5.0, 6.0],
        );
        let source = AstroImage::from_pixels(
            ImageDimensions::new(2, 1, 3),
            vec![10.0, 20.0, 30.0, 40.0, 50.0, 60.0],
        );

        image.apply_from_channel(&source, |_c, dst, src| {
            for (d, s) in dst.iter_mut().zip(src.iter()) {
                *d -= s;
            }
        });

        assert_eq!(image.channel(0).pixels(), &[-9.0, -36.0]);
        assert_eq!(image.channel(1).pixels(), &[-18.0, -45.0]);
        assert_eq!(image.channel(2).pixels(), &[-27.0, -54.0]);
    }

    #[test]
    fn test_image_dimensions_validation() {
        let dims = ImageDimensions::new(100, 200, 3);
        assert_eq!(dims.width, 100);
        assert_eq!(dims.height, 200);
        assert_eq!(dims.channels, 3);
        assert_eq!(dims.pixel_count(), 60000);
        assert!(!dims.is_grayscale());
        assert!(dims.is_rgb());
    }

    #[test]
    #[should_panic(expected = "Width must be positive")]
    fn test_image_dimensions_zero_width() {
        ImageDimensions::new(0, 100, 1);
    }

    #[test]
    #[should_panic(expected = "Height must be positive")]
    fn test_image_dimensions_zero_height() {
        ImageDimensions::new(100, 0, 1);
    }

    #[test]
    #[should_panic(expected = "Only 1 (grayscale) or 3 (RGB) channels supported")]
    fn test_image_dimensions_invalid_channels() {
        ImageDimensions::new(100, 100, 2);
    }

    #[test]
    fn test_bitpix_roundtrip() {
        let values = [
            (8, BitPix::UInt8),
            (16, BitPix::Int16),
            (32, BitPix::Int32),
            (64, BitPix::Int64),
            (-32, BitPix::Float32),
            (-64, BitPix::Float64),
        ];

        for (fits_val, expected) in values {
            let bitpix = BitPix::from_fits_value(fits_val);
            assert_eq!(bitpix, expected);
            assert_eq!(bitpix.to_fits_value(), fits_val);
        }
    }
}
