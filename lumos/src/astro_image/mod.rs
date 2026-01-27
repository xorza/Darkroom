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

/// Represents an astronomical image loaded from a FITS file.
#[derive(Debug, Clone)]
pub struct AstroImage {
    /// Image metadata from FITS headers
    pub metadata: AstroImageMetadata,
    /// Underlying image data (always f32, Gray or RGB)
    pub image: Image,
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

    /// Create a new AstroImage from an ImageDesc and pixel data.
    ///
    /// # Panics
    /// Panics if format is not L_F32 or RGB_F32, or if pixel count doesn't match dimensions.
    pub fn new(desc: ImageDesc, pixels: Vec<f32>) -> Self {
        assert!(
            desc.color_format == ColorFormat::L_F32 || desc.color_format == ColorFormat::RGB_F32,
            "Only L_F32 or RGB_F32 formats supported, got {:?}",
            desc.color_format
        );
        assert_eq!(
            pixels.len(),
            desc.pixel_count(),
            "Pixel count mismatch: expected {}, got {}",
            desc.pixel_count(),
            pixels.len()
        );

        let bytes: Vec<u8> = bytemuck::try_cast_vec(pixels)
            .unwrap_or_else(|(_, pixels)| bytemuck::cast_slice(&pixels).to_vec());
        let image = Image::new_with_data(desc, bytes).expect("Failed to create Image");

        AstroImage {
            metadata: AstroImageMetadata::default(),
            image,
        }
    }

    /// Create a new AstroImage from width, height, channels, and pixel data.
    ///
    /// Convenience constructor that builds the ImageDesc internally.
    ///
    /// # Panics
    /// Panics if channels is not 1 or 3, or if pixel count doesn't match dimensions.
    pub fn from_pixels(width: usize, height: usize, channels: usize, pixels: Vec<f32>) -> Self {
        assert!(
            channels == 1 || channels == 3,
            "Only 1 or 3 channels supported"
        );
        let color_format = if channels == 1 {
            ColorFormat::L_F32
        } else {
            ColorFormat::RGB_F32
        };
        let desc = ImageDesc::new(width, height, color_format);
        Self::new(desc, pixels)
    }

    /// Get image width.
    pub fn width(&self) -> usize {
        self.image.desc().width
    }

    /// Get image height.
    pub fn height(&self) -> usize {
        self.image.desc().height
    }

    /// Get number of channels (1 for grayscale, 3 for RGB).
    pub fn channels(&self) -> usize {
        self.image.desc().color_format.channel_count as usize
    }

    /// Get image dimensions.
    pub fn dimensions(&self) -> ImageDimensions {
        ImageDimensions::new(self.width(), self.height(), self.channels())
    }

    /// Check if this is a grayscale image (1 channel).
    pub fn is_grayscale(&self) -> bool {
        self.channels() == 1
    }

    /// Check if this is an RGB image (3 channels).
    pub fn is_rgb(&self) -> bool {
        self.channels() == 3
    }

    /// Get pixel data as a slice.
    pub fn pixels(&self) -> &[f32] {
        bytemuck::cast_slice(self.image.bytes())
    }

    /// Get pixel data as a mutable slice.
    pub fn pixels_mut(&mut self) -> &mut [f32] {
        bytemuck::cast_slice_mut(self.image.bytes_mut())
    }

    /// Get pixel value at (x, y) for single-channel images.
    ///
    /// This is a clearer name than `get_pixel_gray` for single-channel access.
    ///
    /// # Panics
    /// Panics (in debug builds) if coordinates are out of bounds or if the image
    /// has multiple channels.
    pub fn get_pixel(&self, x: usize, y: usize) -> f32 {
        debug_assert!(x < self.width(), "x coordinate out of bounds");
        debug_assert!(y < self.height(), "y coordinate out of bounds");
        debug_assert!(
            self.is_grayscale(),
            "Use get_pixel_rgb or get_pixel_channel for multi-channel images"
        );

        self.pixels()[y * self.width() + x]
    }

    /// Get pixel value at (x, y) for single-channel images.
    ///
    /// # Deprecated
    /// Use [`get_pixel`](Self::get_pixel) instead for clearer naming.
    ///
    /// # Panics
    /// Panics (in debug builds) if coordinates are out of bounds or if the image
    /// has multiple channels.
    #[deprecated(since = "0.2.0", note = "Use `get_pixel` instead")]
    pub fn get_pixel_gray(&self, x: usize, y: usize) -> f32 {
        self.get_pixel(x, y)
    }

    /// Get pixel value at (x, y) for a specific channel.
    ///
    /// Works for both single-channel and multi-channel images.
    ///
    /// # Panics
    /// Panics (in debug builds) if coordinates or channel index are out of bounds.
    pub fn get_pixel_channel(&self, x: usize, y: usize, channel: usize) -> f32 {
        debug_assert!(x < self.width(), "x coordinate out of bounds");
        debug_assert!(y < self.height(), "y coordinate out of bounds");
        debug_assert!(channel < self.channels(), "channel index out of bounds");

        let idx = (y * self.width() + x) * self.channels() + channel;
        self.pixels()[idx]
    }

    /// Get pixel values at (x, y) for multi-channel images.
    /// Returns a slice of channel values.
    /// Panics if coordinates are out of bounds.
    pub fn get_pixel_rgb(&self, x: usize, y: usize) -> &[f32; 3] {
        debug_assert!(x < self.width(), "x coordinate out of bounds");
        debug_assert!(y < self.height(), "y coordinate out of bounds");
        debug_assert!(self.is_rgb(), "Image must have 3 channels");

        let idx = (y * self.width() + x) * self.channels();
        self.pixels()[idx..idx + 3].as_array::<3>().unwrap()
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
        &mut self.pixels_mut()[idx]
    }

    /// Get mutable reference to pixel values at (x, y) for multi-channel images.
    /// Returns a mutable slice of channel values.
    /// Panics if coordinates are out of bounds.
    pub fn get_pixel_rgb_mut(&mut self, x: usize, y: usize) -> &mut [f32; 3] {
        debug_assert!(x < self.width(), "x coordinate out of bounds");
        debug_assert!(y < self.height(), "y coordinate out of bounds");
        debug_assert!(self.is_rgb(), "Image must have 3 channels");

        let width = self.width();
        let channels = self.channels();
        let idx = (y * width + x) * channels;
        (&mut self.pixels_mut()[idx..idx + 3]).try_into().unwrap()
    }

    /// Get the total number of pixel values (width * height * channels).
    pub fn pixel_count(&self) -> usize {
        self.image.desc().pixel_count()
    }

    /// Calculate the mean pixel value across all pixels using parallel processing.
    pub fn mean(&self) -> f32 {
        let pixels = self.pixels();
        debug_assert!(!pixels.is_empty());
        crate::math::parallel_sum_f32(pixels) / pixels.len() as f32
    }

    /// Convert to grayscale using luminance weights.
    ///
    /// Consumes self. If already grayscale, returns self unchanged.
    /// For RGB images, uses Rec. 709 luminance weights: 0.2126*R + 0.7152*G + 0.0722*B
    pub fn to_grayscale(self) -> Self {
        if self.is_grayscale() {
            return self;
        }

        AstroImage {
            metadata: self.metadata,
            image: self
                .image
                .convert(ColorFormat::L_F32)
                .expect("Failed to convert to grayscale"),
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
        self.image.save_file(path)?;
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
}

impl From<AstroImage> for Image {
    fn from(astro: AstroImage) -> Self {
        astro.image
    }
}

impl From<Image> for AstroImage {
    fn from(image: Image) -> Self {
        let desc = image.desc();

        // Determine target format: Gray or RGB, always f32
        let target_format = match desc.color_format.channel_count {
            ChannelCount::L | ChannelCount::LA => ColorFormat::L_F32,
            ChannelCount::Rgb | ChannelCount::Rgba => ColorFormat::RGB_F32,
        };

        // convert() returns the same image without cloning if format already matches
        let image = image
            .convert(target_format)
            .expect("Failed to convert image to f32")
            .packed();

        // Validate that the resulting image has the expected format
        debug_assert!(
            image.desc().color_format == ColorFormat::L_F32
                || image.desc().color_format == ColorFormat::RGB_F32,
            "Image must be L_F32 or RGB_F32 after conversion"
        );

        AstroImage {
            metadata: AstroImageMetadata::default(),
            image,
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
        let astro = AstroImage::from_pixels(3, 2, 1, vec![0.0, 0.25, 0.5, 0.75, 1.0, 0.5]);

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
            2,
            2,
            3,
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

        // Verify no stride padding (pixels.len() == width * height * channels)
        assert_eq!(image.pixels().len(), image.pixel_count());

        // Test pixel access
        let pixel = image.get_pixel(5, 20);
        assert_eq!(pixel, 152.0);
    }

    #[test]
    fn test_from_image_no_stride_padding() {
        // Create an Image with potential stride padding
        let desc = ImageDesc::new(3, 2, ColorFormat::L_F32);
        let pixels: Vec<f32> = vec![1.0, 2.0, 3.0, 4.0, 5.0, 6.0];
        let bytes: Vec<u8> = bytemuck::cast_slice(&pixels).to_vec();
        let image = Image::new_with_data(desc, bytes).unwrap();

        // Convert to AstroImage
        let astro: AstroImage = image.into();

        // Verify dimensions
        assert_eq!(astro.width(), 3);
        assert_eq!(astro.height(), 2);
        assert_eq!(astro.channels(), 1);

        // Verify no stride padding (pixels.len() == width * height * channels)
        assert_eq!(astro.pixels().len(), astro.pixel_count());
        assert_eq!(astro.pixels().len(), 6);

        // Verify pixel values
        assert_eq!(astro.pixels(), &pixels[..]);
    }

    #[test]
    fn test_mean() {
        let image = AstroImage::from_pixels(2, 2, 1, vec![1.0, 2.0, 3.0, 4.0]);
        assert!((image.mean() - 2.5).abs() < f32::EPSILON);
    }

    #[test]
    fn test_save_grayscale_tiff() {
        let image = AstroImage::from_pixels(2, 2, 1, vec![0.1, 0.2, 0.3, 0.4]);
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
            2,
            2,
            3,
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
        let image = AstroImage::from_pixels(2, 2, 1, vec![0.1, 0.2, 0.3, 0.4]);
        let output_path = test_output_path("astro_save_invalid.xyz");

        let result = image.save(&output_path);
        assert!(result.is_err());
    }

    #[test]
    fn test_calibrate_bias_subtraction() {
        use crate::{CalibrationMasters, StackingMethod};

        let mut light = AstroImage::from_pixels(2, 2, 1, vec![100.0, 200.0, 150.0, 250.0]);
        let bias = AstroImage::from_pixels(2, 2, 1, vec![5.0, 5.0, 5.0, 5.0]);

        let masters = CalibrationMasters {
            master_dark: None,
            master_flat: None,
            master_bias: Some(bias),
            hot_pixel_map: None,
            method: StackingMethod::default(),
        };

        masters.calibrate(&mut light);

        assert_eq!(light.pixels(), &[95.0, 195.0, 145.0, 245.0]);
    }

    #[test]
    fn test_calibrate_dark_subtraction() {
        use crate::{CalibrationMasters, StackingMethod};

        let mut light = AstroImage::from_pixels(2, 2, 1, vec![100.0, 200.0, 150.0, 250.0]);
        let dark = AstroImage::from_pixels(2, 2, 1, vec![10.0, 20.0, 15.0, 25.0]);

        let masters = CalibrationMasters {
            master_dark: Some(dark),
            master_flat: None,
            master_bias: None,
            hot_pixel_map: None,
            method: StackingMethod::default(),
        };

        masters.calibrate(&mut light);

        assert_eq!(light.pixels(), &[90.0, 180.0, 135.0, 225.0]);
    }

    #[test]
    fn test_calibrate_flat_correction() {
        use crate::{CalibrationMasters, StackingMethod};

        let mut light = AstroImage::from_pixels(2, 2, 1, vec![100.0, 200.0, 150.0, 250.0]);
        // Flat with mean = 1.0, so normalized flat equals the flat itself
        let flat = AstroImage::from_pixels(2, 2, 1, vec![0.8, 1.0, 1.2, 1.0]);

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
        assert!((light.pixels()[0] - 125.0).abs() < 0.01);
        assert!((light.pixels()[1] - 200.0).abs() < 0.01);
        assert!((light.pixels()[2] - 125.0).abs() < 0.01);
        assert!((light.pixels()[3] - 250.0).abs() < 0.01);
    }

    #[test]
    fn test_calibrate_full() {
        use crate::{CalibrationMasters, StackingMethod};

        let mut light = AstroImage::from_pixels(2, 2, 1, vec![115.0, 225.0, 170.0, 280.0]);
        let bias = AstroImage::from_pixels(2, 2, 1, vec![5.0, 5.0, 5.0, 5.0]);
        let dark = AstroImage::from_pixels(2, 2, 1, vec![10.0, 20.0, 15.0, 25.0]);
        let flat = AstroImage::from_pixels(2, 2, 1, vec![0.8, 1.0, 1.2, 1.0]);

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
        assert!((light.pixels()[0] - 125.0).abs() < 0.01);
        assert!((light.pixels()[1] - 200.0).abs() < 0.01);
        assert!((light.pixels()[2] - 125.0).abs() < 0.01);
        assert!((light.pixels()[3] - 250.0).abs() < 0.01);
    }

    #[test]
    fn test_roundtrip_astro_to_image_to_astro() {
        let original = AstroImage::from_pixels(3, 2, 1, vec![0.1, 0.2, 0.3, 0.4, 0.5, 0.6]);

        // Convert to Image and back
        let image: Image = original.clone().into();
        let restored: AstroImage = image.into();

        // Verify dimensions preserved
        assert_eq!(restored.dimensions(), original.dimensions());

        // Verify no stride padding
        assert_eq!(restored.pixels().len(), restored.pixel_count());

        // Verify pixel values preserved
        for (a, b) in original.pixels().iter().zip(restored.pixels().iter()) {
            assert!((a - b).abs() < 1e-6, "Pixel mismatch: {} vs {}", a, b);
        }
    }

    #[test]
    fn test_roundtrip_astro_to_image_to_astro_rgb() {
        let original = AstroImage::from_pixels(
            2,
            2,
            3,
            vec![
                1.0, 0.0, 0.0, // red
                0.0, 1.0, 0.0, // green
                0.0, 0.0, 1.0, // blue
                0.5, 0.5, 0.5, // gray
            ],
        );

        // Convert to Image and back
        let image: Image = original.clone().into();
        let restored: AstroImage = image.into();

        // Verify dimensions preserved
        assert_eq!(restored.dimensions(), original.dimensions());

        // Verify pixel values preserved
        for (a, b) in original.pixels().iter().zip(restored.pixels().iter()) {
            assert!((a - b).abs() < 1e-6, "Pixel mismatch: {} vs {}", a, b);
        }
    }

    #[test]
    fn test_image_rgba_to_astro_drops_alpha() {
        // Create RGBA f32 image
        let desc = ImageDesc::new(2, 1, ColorFormat::RGBA_F32);
        let pixels: Vec<f32> = vec![
            1.0, 0.0, 0.0, 0.5, // red with 50% alpha
            0.0, 1.0, 0.0, 1.0, // green with full alpha
        ];
        let bytes: Vec<u8> = bytemuck::cast_slice(&pixels).to_vec();
        let image = Image::new_with_data(desc, bytes).unwrap();

        // Convert to AstroImage (should drop alpha)
        let astro: AstroImage = image.into();

        assert_eq!(astro.channels(), 3);
        assert_eq!(astro.pixels().len(), 6); // 2 pixels * 3 channels

        // Verify RGB values preserved (alpha dropped)
        assert!((astro.pixels()[0] - 1.0).abs() < 1e-6); // R
        assert!((astro.pixels()[1] - 0.0).abs() < 1e-6); // G
        assert!((astro.pixels()[2] - 0.0).abs() < 1e-6); // B
        assert!((astro.pixels()[3] - 0.0).abs() < 1e-6); // R
        assert!((astro.pixels()[4] - 1.0).abs() < 1e-6); // G
        assert!((astro.pixels()[5] - 0.0).abs() < 1e-6); // B
    }

    #[test]
    fn test_image_gray_alpha_to_astro_drops_alpha() {
        // Create LA f32 image
        let desc = ImageDesc::new(2, 1, ColorFormat::LA_F32);
        let pixels: Vec<f32> = vec![
            0.5, 0.8, // gray 0.5 with 80% alpha
            0.9, 1.0, // gray 0.9 with full alpha
        ];
        let bytes: Vec<u8> = bytemuck::cast_slice(&pixels).to_vec();
        let image = Image::new_with_data(desc, bytes).unwrap();

        // Convert to AstroImage (should drop alpha)
        let astro: AstroImage = image.into();

        assert_eq!(astro.channels(), 1);
        assert_eq!(astro.pixels().len(), 2);

        // Verify gray values preserved (alpha dropped)
        assert!((astro.pixels()[0] - 0.5).abs() < 1e-6);
        assert!((astro.pixels()[1] - 0.9).abs() < 1e-6);
    }

    #[test]
    fn test_astro_to_image_preserves_data() {
        let pixels = vec![0.1, 0.2, 0.3, 0.4];
        let astro = AstroImage::from_pixels(2, 2, 1, pixels.clone());

        let image: Image = astro.into();

        // Verify the data is preserved (may or may not be zero-copy depending on alignment)
        let image_floats: &[f32] = bytemuck::cast_slice(image.bytes());
        assert_eq!(image_floats, &pixels[..]);
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
            2,
            2,
            3,
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
            2,
            2,
            3,
            vec![
                100.0, 100.0, 100.0, // Pixel (0,0)
                200.0, 200.0, 200.0, // Pixel (1,0)
                150.0, 150.0, 150.0, // Pixel (0,1)
                250.0, 250.0, 250.0, // Pixel (1,1)
            ],
        );

        // Create RGB bias frame (5.0 for all channels)
        let bias = AstroImage::from_pixels(2, 2, 3, vec![5.0; 12]);

        let masters = CalibrationMasters {
            master_dark: None,
            master_flat: None,
            master_bias: Some(bias),
            hot_pixel_map: None,
            method: StackingMethod::default(),
        };

        masters.calibrate(&mut light);

        // Each channel should have 5.0 subtracted
        assert_eq!(light.pixels()[0], 95.0); // R of (0,0)
        assert_eq!(light.pixels()[1], 95.0); // G of (0,0)
        assert_eq!(light.pixels()[2], 95.0); // B of (0,0)
        assert_eq!(light.pixels()[3], 195.0); // R of (1,0)
    }

    #[test]
    #[should_panic(expected = "don't match")]
    fn test_calibrate_rgb_with_grayscale_bias_panics() {
        use crate::{CalibrationMasters, StackingMethod};

        // RGB light frame
        let mut light = AstroImage::from_pixels(2, 2, 3, vec![100.0; 12]); // 2x2x3

        // Grayscale bias frame (channel mismatch!)
        let bias = AstroImage::from_pixels(2, 2, 1, vec![5.0; 4]); // 2x2x1

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
        let mut light = AstroImage::from_pixels(2, 2, 1, vec![100.0; 4]); // 2x2x1

        // RGB dark frame (channel mismatch!)
        let dark = AstroImage::from_pixels(2, 2, 3, vec![10.0; 12]); // 2x2x3

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
    fn test_get_pixel() {
        let image = AstroImage::from_pixels(3, 2, 1, vec![1.0, 2.0, 3.0, 4.0, 5.0, 6.0]);

        assert_eq!(image.get_pixel(0, 0), 1.0);
        assert_eq!(image.get_pixel(1, 0), 2.0);
        assert_eq!(image.get_pixel(2, 0), 3.0);
        assert_eq!(image.get_pixel(0, 1), 4.0);
        assert_eq!(image.get_pixel(1, 1), 5.0);
        assert_eq!(image.get_pixel(2, 1), 6.0);
    }

    #[test]
    fn test_get_pixel_channel_grayscale() {
        let image = AstroImage::from_pixels(2, 2, 1, vec![1.0, 2.0, 3.0, 4.0]);

        assert_eq!(image.get_pixel_channel(0, 0, 0), 1.0);
        assert_eq!(image.get_pixel_channel(1, 0, 0), 2.0);
        assert_eq!(image.get_pixel_channel(0, 1, 0), 3.0);
        assert_eq!(image.get_pixel_channel(1, 1, 0), 4.0);
    }

    #[test]
    fn test_get_pixel_channel_rgb() {
        let image = AstroImage::from_pixels(
            2,
            2,
            3,
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
    #[allow(deprecated)]
    fn test_get_pixel_gray_deprecated() {
        // Ensure the deprecated function still works
        let image = AstroImage::from_pixels(2, 2, 1, vec![1.0, 2.0, 3.0, 4.0]);
        assert_eq!(image.get_pixel_gray(0, 0), 1.0);
        assert_eq!(image.get_pixel_gray(1, 1), 4.0);
    }
}
