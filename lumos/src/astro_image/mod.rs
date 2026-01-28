pub(crate) mod demosaic;
mod fits;
pub(crate) mod hot_pixels;
pub(crate) mod libraw;
mod sensor;

pub use hot_pixels::HotPixelMap;

use anyhow::Result;
use imaginarium::{ChannelCount, ColorFormat, Image, ImageDesc};
use std::path::Path;

/// FITS BITPIX values representing pixel data types.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum BitPix {
    /// 8-bit unsigned integer (BITPIX = 8)
    #[default]
    UInt8,
    /// 16-bit signed integer (BITPIX = 16)
    Int16,
    /// 32-bit signed integer (BITPIX = 32)
    Int32,
    /// 64-bit signed integer (BITPIX = 64)
    Int64,
    /// 32-bit floating point (BITPIX = -32)
    Float32,
    /// 64-bit floating point (BITPIX = -64)
    Float64,
}

impl BitPix {
    /// Convert from FITS BITPIX integer value.
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

    /// Convert to FITS BITPIX integer value.
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

/// Image dimensions: width, height, and number of channels.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct ImageDimensions {
    /// Image width in pixels
    pub width: usize,
    /// Image height in pixels
    pub height: usize,
    /// Number of channels (1 for grayscale, 3 for RGB)
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

    /// Total number of pixel values (width * height * channels).
    pub fn pixel_count(&self) -> usize {
        self.width * self.height * self.channels
    }

    /// Check if this is a grayscale image (1 channel).
    pub fn is_grayscale(&self) -> bool {
        self.channels == 1
    }

    /// Check if this is an RGB image (3 channels).
    pub fn is_rgb(&self) -> bool {
        self.channels == 3
    }
}

/// Metadata extracted from FITS file headers.
#[derive(Debug, Clone, Default)]
pub struct AstroImageMetadata {
    /// Object name (OBJECT keyword)
    pub object: Option<String>,
    /// Instrument used (INSTRUME keyword)
    pub instrument: Option<String>,
    /// Telescope name (TELESCOP keyword)
    pub telescope: Option<String>,
    /// Observation date (DATE-OBS keyword)
    pub date_obs: Option<String>,
    /// Exposure time in seconds (EXPTIME keyword)
    pub exposure_time: Option<f64>,
    /// ISO sensitivity (from EXIF or raw metadata)
    pub iso: Option<u32>,
    /// Pixel data type (BITPIX keyword)
    pub bitpix: BitPix,
    /// Raw FITS header dimensions [height, width] or [channels, height, width]
    pub header_dimensions: Vec<usize>,
    /// Whether the image has CFA pattern artifacts (from Bayer/X-Trans sensors).
    /// When true, star detection applies median filtering to reduce pattern noise.
    pub is_cfa: bool,
}

/// Pixel data storage - planar format for efficient per-channel operations.
///
/// Astrophotography operations (warping, stacking, calibration) typically process
/// each channel independently. Planar storage makes these operations more efficient
/// by providing contiguous memory access per channel.
#[derive(Debug, Clone)]
pub enum PixelData {
    /// Grayscale image (1 channel) - single Vec of width*height pixels
    L(Vec<f32>),
    /// RGB image (3 channels) - each Vec is width*height pixels
    Rgb([Vec<f32>; 3]),
}

/// Represents an astronomical image loaded from a FITS file.
#[derive(Debug, Clone)]
pub struct AstroImage {
    /// Image metadata from FITS headers
    pub metadata: AstroImageMetadata,
    /// Image dimensions (width, height, channels)
    dimensions: ImageDimensions,
    /// Pixel data in planar format
    pixels: PixelData,
}

// ============================================================================
// Parallel planar/interleaved conversion helpers
// ============================================================================

/// Deinterleave RGB data (RGBRGB...) into separate R, G, B planes.
fn deinterleave_rgb(interleaved: &[f32], r: &mut [f32], g: &mut [f32], b: &mut [f32]) {
    use rayon::prelude::*;

    debug_assert_eq!(interleaved.len(), r.len() * 3);
    debug_assert_eq!(r.len(), g.len());
    debug_assert_eq!(g.len(), b.len());

    const CHUNK_SIZE: usize = 4096;

    // Process all three channels together for better cache locality
    r.par_chunks_mut(CHUNK_SIZE)
        .zip(g.par_chunks_mut(CHUNK_SIZE))
        .zip(b.par_chunks_mut(CHUNK_SIZE))
        .enumerate()
        .for_each(|(chunk_idx, ((r_chunk, g_chunk), b_chunk))| {
            let start = chunk_idx * CHUNK_SIZE;
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

    crate::common::parallel_chunked(interleaved, |i| {
        let pixel_idx = i / 3;
        let channel = i % 3;
        match channel {
            0 => r[pixel_idx],
            1 => g[pixel_idx],
            _ => b[pixel_idx],
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

    crate::common::parallel_chunked(gray, |i| {
        R_WEIGHT * r[i] + G_WEIGHT * g[i] + B_WEIGHT * b[i]
    });
}

impl AstroImage {
    /// Load an astronomical image from a file.
    ///
    /// Automatically detects the file type based on extension:
    /// - FITS files: .fit, .fits
    /// - RAW camera files: .raf, .cr2, .cr3, .nef, .arw, .dng
    /// - Standard image files: .tiff, .tif, .png, .jpg, .jpeg
    ///
    /// # Arguments
    /// * `path` - Path to the image file
    ///
    /// # Returns
    /// * `Result<AstroImage>` - The loaded image or an error
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

    /// Create a new AstroImage from dimensions and interleaved pixel data.
    ///
    /// The pixel data is expected in interleaved format (RGBRGBRGB...) and will be
    /// converted to planar format internally.
    ///
    /// # Panics
    /// Panics if pixel count doesn't match dimensions.
    pub fn from_pixels(dimensions: ImageDimensions, pixels: Vec<f32>) -> Self {
        assert_eq!(
            pixels.len(),
            dimensions.pixel_count(),
            "Pixel count mismatch: expected {}, got {}",
            dimensions.pixel_count(),
            pixels.len()
        );

        let pixel_data = if dimensions.is_grayscale() {
            PixelData::L(pixels)
        } else {
            // Convert interleaved RGB to planar using parallel processing
            let pixel_count = dimensions.width * dimensions.height;
            let mut r = vec![0.0f32; pixel_count];
            let mut g = vec![0.0f32; pixel_count];
            let mut b = vec![0.0f32; pixel_count];

            deinterleave_rgb(&pixels, &mut r, &mut g, &mut b);

            PixelData::Rgb([r, g, b])
        };

        AstroImage {
            metadata: AstroImageMetadata::default(),
            dimensions,
            pixels: pixel_data,
        }
    }

    /// Create an AstroImage from planar channel data.
    ///
    /// Each channel is provided as a separate Vec<f32> in the order [R, G, B] for RGB
    /// or a single channel for grayscale.
    ///
    /// # Panics
    /// Panics if channel count doesn't match dimensions or pixel counts are wrong.
    pub fn from_planar_channels(dimensions: ImageDimensions, channels: Vec<Vec<f32>>) -> Self {
        let expected_pixels_per_channel = dimensions.width * dimensions.height;

        assert_eq!(
            channels.len(),
            dimensions.channels,
            "Channel count mismatch: expected {}, got {}",
            dimensions.channels,
            channels.len()
        );

        for (i, channel) in channels.iter().enumerate() {
            assert_eq!(
                channel.len(),
                expected_pixels_per_channel,
                "Channel {} pixel count mismatch: expected {}, got {}",
                i,
                expected_pixels_per_channel,
                channel.len()
            );
        }

        let pixel_data = if dimensions.is_grayscale() {
            PixelData::L(channels.into_iter().next().unwrap())
        } else {
            let mut iter = channels.into_iter();
            let r = iter.next().unwrap();
            let g = iter.next().unwrap();
            let b = iter.next().unwrap();
            PixelData::Rgb([r, g, b])
        };

        AstroImage {
            metadata: AstroImageMetadata::default(),
            dimensions,
            pixels: pixel_data,
        }
    }

    /// Get image width.
    pub fn width(&self) -> usize {
        self.dimensions.width
    }

    /// Get image height.
    pub fn height(&self) -> usize {
        self.dimensions.height
    }

    /// Get number of channels (1 for grayscale, 3 for RGB).
    pub fn channels(&self) -> usize {
        self.dimensions.channels
    }

    /// Get image dimensions.
    pub fn dimensions(&self) -> ImageDimensions {
        self.dimensions
    }

    /// Check if this is a grayscale image (1 channel).
    pub fn is_grayscale(&self) -> bool {
        matches!(self.pixels, PixelData::L(_))
    }

    /// Check if this is an RGB image (3 channels).
    pub fn is_rgb(&self) -> bool {
        matches!(self.pixels, PixelData::Rgb(_))
    }

    /// Get a single channel's pixel data as a slice.
    ///
    /// For grayscale images, channel 0 is the only valid channel.
    /// For RGB images, channels 0, 1, 2 correspond to R, G, B.
    ///
    /// # Panics
    /// Panics if channel index is out of bounds.
    pub fn channel(&self, c: usize) -> &[f32] {
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

    /// Get a single channel's pixel data as a mutable slice.
    ///
    /// # Panics
    /// Panics if channel index is out of bounds.
    pub fn channel_mut(&mut self, c: usize) -> &mut [f32] {
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

    /// Apply a function to each channel's data, with corresponding source channel.
    ///
    /// Useful for operations like calibration where you need to combine two images.
    /// The function receives `(channel_index, &mut [f32], &[f32])` for each channel.
    pub fn apply_from_channel<F>(&mut self, source: &AstroImage, f: F)
    where
        F: Fn(usize, &mut [f32], &[f32]) + Sync + Send,
    {
        assert_eq!(self.channels(), source.channels(), "Channel count mismatch");
        for c in 0..self.channels() {
            let src = source.channel(c);
            let dst = self.channel_mut(c);
            f(c, dst, src);
        }
    }

    /// Get pixel value at (x, y) for single-channel images.
    ///
    /// This is a clearer name than `get_pixel_gray` for single-channel access.
    ///
    /// # Panics
    /// Panics (in debug builds) if coordinates are out of bounds or if the image
    /// has multiple channels.
    /// todo return correct luma for rgb
    pub fn get_pixel_gray(&self, x: usize, y: usize) -> f32 {
        debug_assert!(x < self.width(), "x coordinate out of bounds");
        debug_assert!(y < self.height(), "y coordinate out of bounds");
        debug_assert!(
            self.is_grayscale(),
            "Use get_pixel_rgb or get_pixel_channel for multi-channel images"
        );

        self.channel(0)[y * self.width() + x]
    }

    /// Get pixel value at (x, y) for a specific channel.
    ///
    /// Works for both single-channel and multi-channel images.
    ///
    /// # Panics
    /// Panics (in debug builds) if coordinates or channel index are out of bounds.
    pub fn get_pixel_channel(&self, x: usize, y: usize, c: usize) -> f32 {
        debug_assert!(x < self.width(), "x coordinate out of bounds");
        debug_assert!(y < self.height(), "y coordinate out of bounds");
        debug_assert!(c < self.channels(), "channel index out of bounds");

        self.channel(c)[y * self.width() + x]
    }

    /// Get pixel values at (x, y) for multi-channel images.
    /// Returns an array of [R, G, B] values.
    /// Panics if coordinates are out of bounds or image is not RGB.
    pub fn get_pixel_rgb(&self, x: usize, y: usize) -> [f32; 3] {
        debug_assert!(x < self.width(), "x coordinate out of bounds");
        debug_assert!(y < self.height(), "y coordinate out of bounds");
        debug_assert!(self.is_rgb(), "Image must have 3 channels");

        let idx = y * self.width() + x;
        match &self.pixels {
            PixelData::Rgb([r, g, b]) => [r[idx], g[idx], b[idx]],
            _ => unreachable!(),
        }
    }

    /// Get mutable reference to pixel value at (x, y) for single-channel images.
    /// Panics if coordinates are out of bounds.
    pub fn get_pixel_gray_mut(&mut self, x: usize, y: usize) -> &mut f32 {
        debug_assert!(x < self.width(), "x coordinate out of bounds");
        debug_assert!(y < self.height(), "y coordinate out of bounds");
        debug_assert!(
            self.is_grayscale(),
            "Use get_pixel_rgb_mut for multi-channel images"
        );

        let width = self.width();
        let idx = y * width + x;
        &mut self.channel_mut(0)[idx]
    }

    /// Set pixel values at (x, y) for multi-channel images.
    /// Panics if coordinates are out of bounds or image is not RGB.
    pub fn set_pixel_rgb(&mut self, x: usize, y: usize, rgb: [f32; 3]) {
        debug_assert!(x < self.width(), "x coordinate out of bounds");
        debug_assert!(y < self.height(), "y coordinate out of bounds");
        debug_assert!(self.is_rgb(), "Image must have 3 channels");

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

    /// Get the total number of pixel values (width * height * channels).
    pub fn pixel_count(&self) -> usize {
        self.dimensions.pixel_count()
    }

    /// Calculate the mean pixel value across all channels using parallel processing.
    pub fn mean(&self) -> f32 {
        match &self.pixels {
            PixelData::L(data) => {
                debug_assert!(!data.is_empty());
                crate::math::parallel_sum_f32(data) / data.len() as f32
            }
            PixelData::Rgb([r, g, b]) => {
                let total = crate::math::parallel_sum_f32(r)
                    + crate::math::parallel_sum_f32(g)
                    + crate::math::parallel_sum_f32(b);
                let count = r.len() + g.len() + b.len();
                total / count as f32
            }
        }
    }

    /// Convert to grayscale using luminance weights.
    ///
    /// Consumes self. If already grayscale, returns self unchanged.
    /// For RGB images, uses Rec. 709 luminance weights: 0.2126*R + 0.7152*G + 0.0722*B
    pub fn to_grayscale(self) -> Self {
        if self.is_grayscale() {
            return self;
        }

        let pixel_count = self.dimensions.width * self.dimensions.height;
        let mut gray = vec![0.0f32; pixel_count];

        match self.pixels {
            PixelData::Rgb([r, g, b]) => {
                rgb_to_luminance(&r, &g, &b, &mut gray);
            }
            _ => unreachable!(),
        }

        AstroImage {
            metadata: self.metadata,
            dimensions: ImageDimensions::new(self.dimensions.width, self.dimensions.height, 1),
            pixels: PixelData::L(gray),
        }
    }

    /// Save the image to a file.
    ///
    /// Automatically detects the file format based on extension:
    /// - PNG: .png
    /// - JPEG: .jpg, .jpeg
    /// - TIFF: .tif, .tiff
    ///
    /// # Arguments
    /// * `path` - Path to save the image to
    ///
    /// # Returns
    /// * `Result<()>` - Ok on success, or an error
    ///
    /// # Example
    /// ```ignore
    /// let image = AstroImage::from_file("input.fits")?;
    /// image.save("output.tiff")?;
    /// ```
    pub fn save<P: AsRef<Path>>(&self, path: P) -> Result<()> {
        // Convert to Image for saving
        let image: Image = self.clone().into();
        image.save_file(path)?;
        Ok(())
    }

    /// Load all astronomical images from a directory.
    ///
    /// Loads all supported image files (RAW and FITS) from the directory
    /// using parallel loading for better performance.
    ///
    /// # Arguments
    /// * `dir` - Path to the directory containing image files
    ///
    /// # Returns
    /// * `Vec<AstroImage>` - Vector of loaded images (empty if directory doesn't exist or has no supported files)
    pub fn load_from_directory<P: AsRef<Path>>(dir: P) -> Vec<Self> {
        use rayon::prelude::*;

        common::file_utils::astro_image_files(dir.as_ref())
            .par_iter()
            .map(|path| Self::from_file(path).expect("Failed to load image"))
            .collect()
    }

    /// Convert to imaginarium::Image, consuming self.
    ///
    /// For grayscale images, this reuses the pixel buffer directly.
    /// For RGB images, converts from planar to interleaved format.
    pub fn into_image(self) -> Image {
        self.into()
    }

    /// Consume self and return pixel data as interleaved Vec<f32> (RGBRGBRGB... format).
    ///
    /// For grayscale images, returns the single channel directly (no copy).
    /// For RGB images, converts from planar to interleaved format.
    pub fn into_interleaved_pixels(self) -> Vec<f32> {
        let pixel_count = self.dimensions.width * self.dimensions.height;
        match self.pixels {
            PixelData::L(data) => data,
            PixelData::Rgb([r, g, b]) => {
                let mut interleaved = vec![0.0f32; pixel_count * 3];
                interleave_rgb(&r, &g, &b, &mut interleaved);
                interleaved
            }
        }
    }
}

impl From<AstroImage> for Image {
    fn from(astro: AstroImage) -> Self {
        let width = astro.dimensions.width;
        let height = astro.dimensions.height;

        let (color_format, interleaved) = match astro.pixels {
            PixelData::L(data) => (ColorFormat::L_F32, data),
            PixelData::Rgb([r, g, b]) => {
                let pixel_count = width * height;
                let mut interleaved = vec![0.0f32; pixel_count * 3];
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
        use rayon::prelude::*;

        let desc = image.desc();

        // Determine target format: Gray or RGB, always f32
        let target_format = match desc.color_format.channel_count {
            ChannelCount::L | ChannelCount::LA => ColorFormat::L_F32,
            ChannelCount::Rgb | ChannelCount::Rgba => ColorFormat::RGB_F32,
        };

        // convert() returns the same image without cloning if format already matches
        let image = image
            .convert(target_format)
            .expect("Failed to convert image to f32");

        let desc = image.desc();
        let width = desc.width;
        let height = desc.height;
        let stride_f32 = desc.stride / std::mem::size_of::<f32>();
        let bytes = image.bytes();

        let pixel_data = if desc.color_format == ColorFormat::L_F32 {
            let pixel_count = width * height;
            let mut data = vec![0.0f32; pixel_count];

            if desc.is_packed() {
                // Fast path: no stride padding, direct copy
                let pixels: &[f32] = bytemuck::cast_slice(bytes);
                data.copy_from_slice(pixels);
            } else {
                // Handle stride: copy row by row in parallel
                let row_bytes = width * std::mem::size_of::<f32>();
                data.par_chunks_mut(width).enumerate().for_each(|(y, row)| {
                    let src_offset = y * desc.stride;
                    let src_row: &[f32] =
                        bytemuck::cast_slice(&bytes[src_offset..src_offset + row_bytes]);
                    row.copy_from_slice(src_row);
                });
            }
            PixelData::L(data)
        } else {
            let pixel_count = width * height;
            let mut r = vec![0.0f32; pixel_count];
            let mut g = vec![0.0f32; pixel_count];
            let mut b = vec![0.0f32; pixel_count];

            if desc.is_packed() {
                // Fast path: use parallel deinterleave
                let pixels: &[f32] = bytemuck::cast_slice(bytes);
                deinterleave_rgb(pixels, &mut r, &mut g, &mut b);
            } else {
                // Handle stride: process row by row in parallel
                let pixels: &[f32] = bytemuck::cast_slice(bytes);
                r.par_chunks_mut(width)
                    .zip(g.par_chunks_mut(width))
                    .zip(b.par_chunks_mut(width))
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
            }
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

        // Verify pixel data
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

        // Verify pixel data
        let pixels: &[f32] = bytemuck::cast_slice(image.bytes());
        assert_eq!(pixels.len(), 12);
        // First pixel (red)
        assert_eq!(pixels[0], 1.0);
        assert_eq!(pixels[1], 0.0);
        assert_eq!(pixels[2], 0.0);
        // Last pixel (white)
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

        // Test pixel access
        let pixel = image.get_pixel_gray(5, 20);
        assert_eq!(pixel, 152.0);
    }

    #[test]
    fn test_from_image_no_stride_padding() {
        // Create an Image with potential stride padding
        let desc = ImageDesc::new_with_stride(3, 2, ColorFormat::L_F32);
        let pixels: Vec<f32> = vec![1.0, 2.0, 3.0, 4.0, 5.0, 6.0];
        let bytes: Vec<u8> = bytemuck::cast_slice(&pixels).to_vec();
        let image = Image::new_with_data(desc, bytes).unwrap();

        // Convert to AstroImage
        let astro: AstroImage = image.into();

        // Verify dimensions
        assert_eq!(astro.width(), 3);
        assert_eq!(astro.height(), 2);
        assert_eq!(astro.channels(), 1);

        // Verify pixel values
        assert_eq!(astro.channel(0), &pixels[..]);
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

        // Verify we can load it back
        let loaded = AstroImage::from_file(&output_path).unwrap();
        assert_eq!(loaded.width(), 2);
        assert_eq!(loaded.height(), 2);
        assert_eq!(loaded.channels(), 1);
    }

    #[test]
    fn test_save_rgb_tiff() {
        let image = AstroImage::from_pixels(
            ImageDimensions::new(2, 2, 3),
            vec![
                1.0, 0.0, 0.0, // red
                0.0, 1.0, 0.0, // green
                0.0, 0.0, 1.0, // blue
                1.0, 1.0, 1.0, // white
            ],
        );
        let output_path = test_output_path("astro_save_rgb.tiff");

        image.save(&output_path).unwrap();

        assert!(output_path.exists());

        // Verify we can load it back
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

        assert_eq!(light.channel(0), &[95.0, 195.0, 145.0, 245.0]);
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

        assert_eq!(light.channel(0), &[90.0, 180.0, 135.0, 225.0]);
    }

    #[test]
    fn test_calibrate_flat_correction() {
        use crate::{CalibrationMasters, StackingMethod};

        let mut light = AstroImage::from_pixels(
            ImageDimensions::new(2, 2, 1),
            vec![100.0, 200.0, 150.0, 250.0],
        );
        // Flat with mean = 1.0, so normalized flat equals the flat itself
        let flat = AstroImage::from_pixels(ImageDimensions::new(2, 2, 1), vec![0.8, 1.0, 1.2, 1.0]);

        let masters = CalibrationMasters {
            master_dark: None,
            master_flat: Some(flat),
            master_bias: None,
            hot_pixel_map: None,
            method: StackingMethod::default(),
        };

        masters.calibrate(&mut light);

        // Each pixel divided by (flat_pixel / flat_mean)
        // flat_mean = 1.0, so: 100/0.8=125, 200/1.0=200, 150/1.2=125, 250/1.0=250
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

        // After bias: [110, 220, 165, 275]
        // After dark: [100, 200, 150, 250]
        // After flat (mean=1.0): [125, 200, 125, 250]
        assert!((light.channel(0)[0] - 125.0).abs() < 0.01);
        assert!((light.channel(0)[1] - 200.0).abs() < 0.01);
        assert!((light.channel(0)[2] - 125.0).abs() < 0.01);
        assert!((light.channel(0)[3] - 250.0).abs() < 0.01);
    }

    #[test]
    fn test_roundtrip_astro_to_image_to_astro() {
        // Test grayscale roundtrip
        let gray = AstroImage::from_pixels(
            ImageDimensions::new(3, 2, 1),
            vec![0.1, 0.2, 0.3, 0.4, 0.5, 0.6],
        );

        let image: Image = gray.clone().into();
        let restored: AstroImage = image.into();

        assert_eq!(restored.dimensions(), gray.dimensions());
        for (a, b) in gray.channel(0).iter().zip(restored.channel(0).iter()) {
            assert!(
                (a - b).abs() < 1e-6,
                "Grayscale pixel mismatch: {} vs {}",
                a,
                b
            );
        }

        // Test RGB roundtrip
        let rgb = AstroImage::from_pixels(
            ImageDimensions::new(2, 2, 3),
            vec![
                1.0, 0.0, 0.0, // red
                0.0, 1.0, 0.0, // green
                0.0, 0.0, 1.0, // blue
                0.5, 0.5, 0.5, // gray
            ],
        );

        let image: Image = rgb.clone().into();
        let restored: AstroImage = image.into();

        assert_eq!(restored.dimensions(), rgb.dimensions());
        for c in 0..rgb.channels() {
            for (a, b) in rgb.channel(c).iter().zip(restored.channel(c).iter()) {
                assert!((a - b).abs() < 1e-6, "RGB pixel mismatch: {} vs {}", a, b);
            }
        }
    }

    #[test]
    fn test_image_rgba_to_astro_drops_alpha() {
        // Create RGBA f32 image
        let desc = ImageDesc::new_with_stride(2, 1, ColorFormat::RGBA_F32);
        let pixels: Vec<f32> = vec![
            1.0, 0.0, 0.0, 0.5, // red with 50% alpha
            0.0, 1.0, 0.0, 1.0, // green with full alpha
        ];
        let bytes: Vec<u8> = bytemuck::cast_slice(&pixels).to_vec();
        let image = Image::new_with_data(desc, bytes).unwrap();

        // Convert to AstroImage (should drop alpha)
        let astro: AstroImage = image.into();

        assert_eq!(astro.channels(), 3);

        // Verify RGB values preserved (alpha dropped)
        // Pixel 0: R=1.0, G=0.0, B=0.0
        // Pixel 1: R=0.0, G=1.0, B=0.0
        assert!((astro.channel(0)[0] - 1.0).abs() < 1e-6); // R pixel 0
        assert!((astro.channel(1)[0] - 0.0).abs() < 1e-6); // G pixel 0
        assert!((astro.channel(2)[0] - 0.0).abs() < 1e-6); // B pixel 0
        assert!((astro.channel(0)[1] - 0.0).abs() < 1e-6); // R pixel 1
        assert!((astro.channel(1)[1] - 1.0).abs() < 1e-6); // G pixel 1
        assert!((astro.channel(2)[1] - 0.0).abs() < 1e-6); // B pixel 1
    }

    #[test]
    fn test_image_gray_alpha_to_astro_drops_alpha() {
        // Create LA f32 image
        let desc = ImageDesc::new_with_stride(2, 1, ColorFormat::LA_F32);
        let pixels: Vec<f32> = vec![
            0.5, 0.8, // gray 0.5 with 80% alpha
            0.9, 1.0, // gray 0.9 with full alpha
        ];
        let bytes: Vec<u8> = bytemuck::cast_slice(&pixels).to_vec();
        let image = Image::new_with_data(desc, bytes).unwrap();

        // Convert to AstroImage (should drop alpha)
        let astro: AstroImage = image.into();

        assert_eq!(astro.channels(), 1);

        // Verify gray values preserved (alpha dropped)
        assert!((astro.channel(0)[0] - 0.5).abs() < 1e-6);
        assert!((astro.channel(0)[1] - 0.9).abs() < 1e-6);
    }

    #[test]
    #[cfg_attr(not(feature = "slow-tests"), ignore)]
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

        // Get first file directly without parallel loading
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
        // RGB images have 3 channels
        assert_eq!(image.channels(), 3);

        let image: imaginarium::Image = image.into();

        image
            .save_file(test_output_path("light_from_raw.tiff"))
            .unwrap();
    }

    #[test]
    #[cfg_attr(not(feature = "slow-tests"), ignore)]
    fn test_calibrate_light_from_env() {
        use crate::testing::{calibration_dir, calibration_masters_dir, init_tracing};
        use crate::{CalibrationMasters, StackingMethod};

        init_tracing();

        // Load first light frame directly (not all files - libraw is slow)
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

        // Load calibration masters
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

        // Calibrate the light frame
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

        // Save calibrated image to output
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
        // Create a 2x2 RGB image (3 channels)
        // RGB pixels: R, G, B for each pixel, row-major order
        // Pixel (0,0): R=10, G=20, B=30
        // Pixel (1,0): R=40, G=50, B=60
        // Pixel (0,1): R=70, G=80, B=90
        // Pixel (1,1): R=100, G=110, B=120
        let image = AstroImage::from_pixels(
            ImageDimensions::new(2, 2, 3),
            vec![
                10.0, 20.0, 30.0, 40.0, 50.0, 60.0, 70.0, 80.0, 90.0, 100.0, 110.0, 120.0,
            ],
        );

        // Verify dimensions
        assert_eq!(image.width(), 2);
        assert_eq!(image.height(), 2);
        assert_eq!(image.channels(), 3);
        assert!(!image.is_grayscale());
        // pixel_count() returns total values (width * height * channels)
        assert_eq!(image.pixel_count(), 12); // 2x2x3
        assert_eq!(image.dimensions().pixel_count(), 12);

        // Verify mean calculation
        let expected_mean: f32 =
            (10.0 + 20.0 + 30.0 + 40.0 + 50.0 + 60.0 + 70.0 + 80.0 + 90.0 + 100.0 + 110.0 + 120.0)
                / 12.0;
        assert!((image.mean() - expected_mean).abs() < f32::EPSILON);
    }

    #[test]
    fn test_calibrate_rgb_with_rgb_masters() {
        use crate::{CalibrationMasters, StackingMethod};

        // Create 2x2 RGB light frame
        let mut light = AstroImage::from_pixels(
            ImageDimensions::new(2, 2, 3),
            vec![
                100.0, 100.0, 100.0, // Pixel (0,0)
                200.0, 200.0, 200.0, // Pixel (1,0)
                150.0, 150.0, 150.0, // Pixel (0,1)
                250.0, 250.0, 250.0, // Pixel (1,1)
            ],
        );

        // Create RGB bias frame (5.0 for all channels)
        let bias = AstroImage::from_pixels(ImageDimensions::new(2, 2, 3), vec![5.0; 12]);

        let masters = CalibrationMasters {
            master_dark: None,
            master_flat: None,
            master_bias: Some(bias),
            hot_pixel_map: None,
            method: StackingMethod::default(),
        };

        masters.calibrate(&mut light);

        // Each channel should have 5.0 subtracted
        assert_eq!(light.channel(0)[0], 95.0); // R of (0,0)
        assert_eq!(light.channel(1)[0], 95.0); // G of (0,0)
        assert_eq!(light.channel(2)[0], 95.0); // B of (0,0)
        assert_eq!(light.channel(0)[1], 195.0); // R of (1,0)
    }

    #[test]
    #[should_panic(expected = "don't match")]
    fn test_calibrate_rgb_with_grayscale_bias_panics() {
        use crate::{CalibrationMasters, StackingMethod};

        // RGB light frame
        let mut light = AstroImage::from_pixels(ImageDimensions::new(2, 2, 3), vec![100.0; 12]); // 2x2x3

        // Grayscale bias frame (channel mismatch!)
        let bias = AstroImage::from_pixels(ImageDimensions::new(2, 2, 1), vec![5.0; 4]); // 2x2x1

        let masters = CalibrationMasters {
            master_dark: None,
            master_flat: None,
            master_bias: Some(bias),
            hot_pixel_map: None,
            method: StackingMethod::default(),
        };

        // This should panic due to dimension mismatch (channels differ)
        masters.calibrate(&mut light);
    }

    #[test]
    #[should_panic(expected = "don't match")]
    fn test_calibrate_grayscale_with_rgb_dark_panics() {
        use crate::{CalibrationMasters, StackingMethod};

        // Grayscale light frame
        let mut light = AstroImage::from_pixels(ImageDimensions::new(2, 2, 1), vec![100.0; 4]); // 2x2x1

        // RGB dark frame (channel mismatch!)
        let dark = AstroImage::from_pixels(ImageDimensions::new(2, 2, 3), vec![10.0; 12]); // 2x2x3

        let masters = CalibrationMasters {
            master_dark: Some(dark),
            master_flat: None,
            master_bias: None,
            hot_pixel_map: None,
            method: StackingMethod::default(),
        };

        // This should panic due to dimension mismatch (channels differ)
        masters.calibrate(&mut light);
    }

    #[test]
    fn test_get_pixel_gray() {
        let image = AstroImage::from_pixels(
            ImageDimensions::new(3, 2, 1),
            vec![1.0, 2.0, 3.0, 4.0, 5.0, 6.0],
        );

        // Test all pixels in row-major order
        assert_eq!(image.get_pixel_gray(0, 0), 1.0);
        assert_eq!(image.get_pixel_gray(2, 0), 3.0);
        assert_eq!(image.get_pixel_gray(0, 1), 4.0);
        assert_eq!(image.get_pixel_gray(2, 1), 6.0);

        // get_pixel_channel should work the same for grayscale
        assert_eq!(image.get_pixel_channel(1, 0, 0), 2.0);
        assert_eq!(image.get_pixel_channel(1, 1, 0), 5.0);
    }

    #[test]
    fn test_get_pixel_channel_rgb() {
        let image = AstroImage::from_pixels(
            ImageDimensions::new(2, 2, 3),
            vec![
                1.0, 2.0, 3.0, // (0,0): R=1, G=2, B=3
                4.0, 5.0, 6.0, // (1,0): R=4, G=5, B=6
                7.0, 8.0, 9.0, // (0,1): R=7, G=8, B=9
                10.0, 11.0, 12.0, // (1,1): R=10, G=11, B=12
            ],
        );

        // Test pixel (0,0)
        assert_eq!(image.get_pixel_channel(0, 0, 0), 1.0); // R
        assert_eq!(image.get_pixel_channel(0, 0, 1), 2.0); // G
        assert_eq!(image.get_pixel_channel(0, 0, 2), 3.0); // B

        // Test pixel (1,0)
        assert_eq!(image.get_pixel_channel(1, 0, 0), 4.0); // R
        assert_eq!(image.get_pixel_channel(1, 0, 1), 5.0); // G
        assert_eq!(image.get_pixel_channel(1, 0, 2), 6.0); // B

        // Test pixel (0,1)
        assert_eq!(image.get_pixel_channel(0, 1, 0), 7.0); // R
        assert_eq!(image.get_pixel_channel(0, 1, 1), 8.0); // G
        assert_eq!(image.get_pixel_channel(0, 1, 2), 9.0); // B

        // Test pixel (1,1)
        assert_eq!(image.get_pixel_channel(1, 1, 0), 10.0); // R
        assert_eq!(image.get_pixel_channel(1, 1, 1), 11.0); // G
        assert_eq!(image.get_pixel_channel(1, 1, 2), 12.0); // B
    }

    #[test]
    fn test_to_grayscale_rgb() {
        let rgb = AstroImage::from_pixels(
            ImageDimensions::new(2, 1, 3),
            vec![
                1.0, 0.0, 0.0, // red -> 0.2126
                0.0, 1.0, 0.0, // green -> 0.7152
            ],
        );

        let gray = rgb.to_grayscale();

        assert!(gray.is_grayscale());
        assert_eq!(gray.channels(), 1);
        assert!((gray.channel(0)[0] - 0.2126).abs() < 1e-4);
        assert!((gray.channel(0)[1] - 0.7152).abs() < 1e-4);
    }

    #[test]
    fn test_to_grayscale_already_gray() {
        let gray = AstroImage::from_pixels(ImageDimensions::new(2, 2, 1), vec![1.0, 2.0, 3.0, 4.0]);
        let result = gray.to_grayscale();

        assert!(result.is_grayscale());
        assert_eq!(result.channel(0), &[1.0, 2.0, 3.0, 4.0]);
    }

    #[test]
    fn test_from_planar_channels_grayscale() {
        let channels = vec![vec![1.0, 2.0, 3.0, 4.0]];
        let image = AstroImage::from_planar_channels(ImageDimensions::new(2, 2, 1), channels);

        assert!(image.is_grayscale());
        assert_eq!(image.channel(0), &[1.0, 2.0, 3.0, 4.0]);
    }

    #[test]
    fn test_from_planar_channels_rgb() {
        let r = vec![1.0, 2.0];
        let g = vec![3.0, 4.0];
        let b = vec![5.0, 6.0];
        let image = AstroImage::from_planar_channels(ImageDimensions::new(2, 1, 3), vec![r, g, b]);

        assert!(image.is_rgb());
        assert_eq!(image.channel(0), &[1.0, 2.0]); // R
        assert_eq!(image.channel(1), &[3.0, 4.0]); // G
        assert_eq!(image.channel(2), &[5.0, 6.0]); // B
    }

    #[test]
    fn test_channel_mut() {
        let mut image =
            AstroImage::from_pixels(ImageDimensions::new(2, 2, 1), vec![1.0, 2.0, 3.0, 4.0]);

        image.channel_mut(0)[0] = 10.0;
        image.channel_mut(0)[3] = 40.0;

        assert_eq!(image.channel(0), &[10.0, 2.0, 3.0, 40.0]);
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

        // Subtract source from image
        image.apply_from_channel(&source, |_c, dst, src| {
            for (d, s) in dst.iter_mut().zip(src.iter()) {
                *d -= s;
            }
        });

        assert_eq!(image.channel(0), &[-9.0, -36.0]); // R: 1-10, 4-40
        assert_eq!(image.channel(1), &[-18.0, -45.0]); // G: 2-20, 5-50
        assert_eq!(image.channel(2), &[-27.0, -54.0]); // B: 3-30, 6-60
    }

    #[test]
    fn test_image_dimensions_validation() {
        // Valid dimensions
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
