mod fits;
mod libraw;
mod rawloader;

use anyhow::Result;
use imaginarium::{ChannelCount, ChannelSize, ChannelType, ColorFormat, Image, ImageDesc};
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
        assert!(channels > 0, "Channels must be positive");
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
    /// Pixel data type (BITPIX keyword)
    pub bitpix: BitPix,
    /// Raw FITS header dimensions [height, width] or [channels, height, width]
    pub header_dimensions: Vec<usize>,
}

/// Represents an astronomical image loaded from a FITS file.
#[derive(Debug)]
pub struct AstroImage {
    /// Image metadata from FITS headers
    pub metadata: AstroImageMetadata,
    /// Pixel data stored as f32 for processing flexibility
    pub pixels: Vec<f32>,
    /// Image dimensions
    pub dimensions: ImageDimensions,
}

impl AstroImage {
    /// Load an astronomical image from a file.
    ///
    /// Automatically detects the file type based on extension:
    /// - FITS files: .fit, .fits
    /// - RAW camera files: .raf, .cr2, .cr3, .nef, .arw, .dng
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
            "raf" | "cr2" | "cr3" | "nef" | "arw" | "dng" => {
                // Try rawloader first (faster, pure Rust), fall back to libraw
                rawloader::load_raw(path).or_else(|_| libraw::load_raw(path))
            }
            _ => anyhow::bail!("Unsupported file extension: {}", ext),
        }
    }

    /// Get pixel value at (x, y) for single-channel images.
    /// Panics if coordinates are out of bounds.
    pub fn get_pixel_gray(&self, x: usize, y: usize) -> f32 {
        debug_assert!(x < self.dimensions.width, "x coordinate out of bounds");
        debug_assert!(y < self.dimensions.height, "y coordinate out of bounds");
        debug_assert!(
            self.dimensions.is_grayscale(),
            "Use get_pixel_rgb for multi-channel images"
        );

        self.pixels[y * self.dimensions.width + x]
    }

    /// Get pixel values at (x, y) for multi-channel images.
    /// Returns a slice of channel values.
    /// Panics if coordinates are out of bounds.
    pub fn get_pixel_rgb(&self, x: usize, y: usize) -> &[f32; 3] {
        debug_assert!(x < self.dimensions.width, "x coordinate out of bounds");
        debug_assert!(y < self.dimensions.height, "y coordinate out of bounds");
        debug_assert!(self.dimensions.is_rgb(), "Image must have 3 channels");

        let idx = (y * self.dimensions.width + x) * self.dimensions.channels;
        self.pixels[idx..idx + 3].as_array::<3>().unwrap()
    }

    /// Get mutable reference to pixel value at (x, y) for single-channel images.
    /// Panics if coordinates are out of bounds.
    pub fn get_pixel_gray_mut(&mut self, x: usize, y: usize) -> &mut f32 {
        debug_assert!(x < self.dimensions.width, "x coordinate out of bounds");
        debug_assert!(y < self.dimensions.height, "y coordinate out of bounds");
        debug_assert!(
            self.dimensions.is_grayscale(),
            "Use get_pixel_rgb_mut for multi-channel images"
        );

        let idx = y * self.dimensions.width + x;
        &mut self.pixels[idx]
    }

    /// Get mutable reference to pixel values at (x, y) for multi-channel images.
    /// Returns a mutable slice of channel values.
    /// Panics if coordinates are out of bounds.
    pub fn get_pixel_rgb_mut(&mut self, x: usize, y: usize) -> &mut [f32; 3] {
        debug_assert!(x < self.dimensions.width, "x coordinate out of bounds");
        debug_assert!(y < self.dimensions.height, "y coordinate out of bounds");
        debug_assert!(self.dimensions.is_rgb(), "Image must have 3 channels");

        let idx = (y * self.dimensions.width + x) * self.dimensions.channels;
        (&mut self.pixels[idx..idx + 3]).try_into().unwrap()
    }

    /// Get the total number of pixels (width * height * channels).
    pub fn pixel_count(&self) -> usize {
        self.dimensions.pixel_count()
    }
}

impl From<AstroImage> for Image {
    fn from(astro: AstroImage) -> Self {
        let channel_count = match astro.dimensions.channels {
            1 => ChannelCount::Gray,
            3 => ChannelCount::Rgb,
            _ => panic!("Unsupported channel count: {}", astro.dimensions.channels),
        };

        let color_format = ColorFormat {
            channel_count,
            channel_size: ChannelSize::_32bit,
            channel_type: ChannelType::Float,
        };

        let desc = ImageDesc::new(
            astro.dimensions.width,
            astro.dimensions.height,
            color_format,
        );

        let bytes: Vec<u8> = bytemuck::cast_slice(&astro.pixels).to_vec();

        Image::new_with_data(desc, bytes).expect("Failed to create Image from AstroImage")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_metadata_default() {
        let meta = AstroImageMetadata::default();
        assert!(meta.object.is_none());
        assert!(meta.header_dimensions.is_empty());
    }

    #[test]
    fn test_convert_to_imaginarium_image_grayscale() {
        let astro = AstroImage {
            metadata: AstroImageMetadata::default(),
            pixels: vec![0.0, 0.25, 0.5, 0.75, 1.0, 0.5],
            dimensions: ImageDimensions::new(3, 2, 1),
        };

        let image: Image = astro.into();
        let desc = image.desc();

        assert_eq!(desc.width, 3);
        assert_eq!(desc.height, 2);
        assert_eq!(desc.color_format.channel_count, ChannelCount::Gray);
        assert_eq!(desc.color_format.channel_size, ChannelSize::_32bit);
        assert_eq!(desc.color_format.channel_type, ChannelType::Float);

        // Verify pixel data
        let pixels: &[f32] = bytemuck::cast_slice(image.bytes());
        assert_eq!(pixels.len(), 6);
        assert_eq!(pixels[0], 0.0);
        assert_eq!(pixels[1], 0.25);
        assert_eq!(pixels[4], 1.0);
    }

    #[test]
    fn test_convert_to_imaginarium_image_rgb() {
        let astro = AstroImage {
            metadata: AstroImageMetadata::default(),
            pixels: vec![
                1.0, 0.0, 0.0, // red
                0.0, 1.0, 0.0, // green
                0.0, 0.0, 1.0, // blue
                1.0, 1.0, 1.0, // white
            ],
            dimensions: ImageDimensions::new(2, 2, 3),
        };

        let image: Image = astro.into();
        let desc = image.desc();

        assert_eq!(desc.width, 2);
        assert_eq!(desc.height, 2);
        assert_eq!(desc.color_format.channel_count, ChannelCount::Rgb);
        assert_eq!(desc.color_format.channel_size, ChannelSize::_32bit);
        assert_eq!(desc.color_format.channel_type, ChannelType::Float);

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
        assert_eq!(desc.color_format, ColorFormat::GRAY_F32);
    }

    #[test]
    fn test_load_full_example_fits() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../test_resources/full_example.fits"
        );
        let image = AstroImage::from_file(path).unwrap();

        assert_eq!(image.dimensions.width, 100);
        assert_eq!(image.dimensions.height, 100);
        assert_eq!(image.dimensions.channels, 1);
        assert!(image.dimensions.is_grayscale());
        assert_eq!(image.pixel_count(), 10000);
        assert_eq!(image.metadata.bitpix, BitPix::Int32);
        assert_eq!(image.metadata.header_dimensions, vec![100, 100]);

        // Test pixel access
        let pixel = image.get_pixel_gray(5, 20);
        assert_eq!(pixel, 152.0);
    }

    #[test]
    #[cfg_attr(not(feature = "slow-tests"), ignore)]
    fn test_load_single_raw_from_env() {
        use crate::test_utils::load_calibration_images;

        let Some(images) = load_calibration_images("Darks") else {
            eprintln!("LUMOS_CALIBRATION_DIR not set or Darks dir missing, skipping test");
            return;
        };

        let Some(image) = images.into_iter().next() else {
            eprintln!("No raw files in Darks, skipping test");
            return;
        };

        println!(
            "Loaded image: {}x{}x{}",
            image.dimensions.width, image.dimensions.height, image.dimensions.channels
        );

        assert!(image.dimensions.width > 0);
        assert!(image.dimensions.height > 0);
        assert_eq!(image.dimensions.channels, 1);
        assert_eq!(
            image.pixels.len(),
            image.dimensions.width * image.dimensions.height
        );
    }
}
