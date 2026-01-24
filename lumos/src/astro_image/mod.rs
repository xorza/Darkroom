pub(crate) mod demosaic;
mod fits;
pub(crate) mod hot_pixels;
pub(crate) mod libraw;
mod sensor;

pub use hot_pixels::HotPixelMap;

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
#[derive(Debug, Clone)]
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
            "raf" | "cr2" | "cr3" | "nef" | "arw" | "dng" => libraw::load_raw(path),
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

    /// Calculate the mean pixel value across all pixels using parallel processing.
    pub fn mean(&self) -> f32 {
        debug_assert!(!self.pixels.is_empty());
        crate::math::parallel_sum_f32(&self.pixels) / self.pixels.len() as f32
    }

    /// Convert to grayscale using luminance weights.
    ///
    /// If already grayscale, returns a clone.
    /// For RGB images, uses standard luminance weights: 0.2126*R + 0.7152*G + 0.0722*B
    pub fn to_grayscale(&self) -> Self {
        if self.dimensions.is_grayscale() {
            return self.clone();
        }

        //todo implement directly
        let image: Image = self.clone().into();
        image
            .convert(ColorFormat::GRAY_F32)
            .expect("Failed to convert to grayscale")
            .into()
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

impl From<Image> for AstroImage {
    fn from(image: Image) -> Self {
        let desc = image.desc();

        // Determine target format: Gray or RGB, always f32
        let (target_format, channels) = match desc.color_format.channel_count {
            ChannelCount::Gray | ChannelCount::GrayAlpha => (ColorFormat::GRAY_F32, 1),
            ChannelCount::Rgb | ChannelCount::Rgba => (ColorFormat::RGB_F32, 3),
        };

        let image = image
            .convert(target_format)
            .expect("Failed to convert image to f32")
            .packed();

        let pixels: Vec<f32> = bytemuck::cast_slice(image.bytes()).to_vec();

        AstroImage {
            metadata: AstroImageMetadata::default(),
            pixels,
            dimensions: ImageDimensions::new(image.desc().width, image.desc().height, channels),
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

        // Verify no stride padding (pixels.len() == width * height * channels)
        assert_eq!(image.pixels.len(), image.dimensions.pixel_count());

        // Test pixel access
        let pixel = image.get_pixel_gray(5, 20);
        assert_eq!(pixel, 152.0);
    }

    #[test]
    fn test_from_image_no_stride_padding() {
        // Create an Image with potential stride padding
        let desc = ImageDesc::new(3, 2, ColorFormat::GRAY_F32);
        let pixels: Vec<f32> = vec![1.0, 2.0, 3.0, 4.0, 5.0, 6.0];
        let bytes: Vec<u8> = bytemuck::cast_slice(&pixels).to_vec();
        let image = Image::new_with_data(desc, bytes).unwrap();

        // Convert to AstroImage
        let astro: AstroImage = image.into();

        // Verify dimensions
        assert_eq!(astro.dimensions.width, 3);
        assert_eq!(astro.dimensions.height, 2);
        assert_eq!(astro.dimensions.channels, 1);

        // Verify no stride padding (pixels.len() == width * height * channels)
        assert_eq!(astro.pixels.len(), astro.dimensions.pixel_count());
        assert_eq!(astro.pixels.len(), 6);

        // Verify pixel values
        assert_eq!(astro.pixels, pixels);
    }

    #[test]
    fn test_mean() {
        let image = AstroImage {
            metadata: AstroImageMetadata::default(),
            pixels: vec![1.0, 2.0, 3.0, 4.0],
            dimensions: ImageDimensions::new(2, 2, 1),
        };
        assert!((image.mean() - 2.5).abs() < f32::EPSILON);
    }

    #[test]
    fn test_calibrate_bias_subtraction() {
        use crate::{CalibrationMasters, StackingMethod};

        let mut light = AstroImage {
            metadata: AstroImageMetadata::default(),
            pixels: vec![100.0, 200.0, 150.0, 250.0],
            dimensions: ImageDimensions::new(2, 2, 1),
        };
        let bias = AstroImage {
            metadata: AstroImageMetadata::default(),
            pixels: vec![5.0, 5.0, 5.0, 5.0],
            dimensions: ImageDimensions::new(2, 2, 1),
        };

        let masters = CalibrationMasters {
            master_dark: None,
            master_flat: None,
            master_bias: Some(bias),
            hot_pixel_map: None,
            method: StackingMethod::default(),
        };

        masters.calibrate(&mut light);

        assert_eq!(light.pixels, vec![95.0, 195.0, 145.0, 245.0]);
    }

    #[test]
    fn test_calibrate_dark_subtraction() {
        use crate::{CalibrationMasters, StackingMethod};

        let mut light = AstroImage {
            metadata: AstroImageMetadata::default(),
            pixels: vec![100.0, 200.0, 150.0, 250.0],
            dimensions: ImageDimensions::new(2, 2, 1),
        };
        let dark = AstroImage {
            metadata: AstroImageMetadata::default(),
            pixels: vec![10.0, 20.0, 15.0, 25.0],
            dimensions: ImageDimensions::new(2, 2, 1),
        };

        let masters = CalibrationMasters {
            master_dark: Some(dark),
            master_flat: None,
            master_bias: None,
            hot_pixel_map: None,
            method: StackingMethod::default(),
        };

        masters.calibrate(&mut light);

        assert_eq!(light.pixels, vec![90.0, 180.0, 135.0, 225.0]);
    }

    #[test]
    fn test_calibrate_flat_correction() {
        use crate::{CalibrationMasters, StackingMethod};

        let mut light = AstroImage {
            metadata: AstroImageMetadata::default(),
            pixels: vec![100.0, 200.0, 150.0, 250.0],
            dimensions: ImageDimensions::new(2, 2, 1),
        };
        // Flat with mean = 1.0, so normalized flat equals the flat itself
        let flat = AstroImage {
            metadata: AstroImageMetadata::default(),
            pixels: vec![0.8, 1.0, 1.2, 1.0],
            dimensions: ImageDimensions::new(2, 2, 1),
        };

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
        assert!((light.pixels[0] - 125.0).abs() < 0.01);
        assert!((light.pixels[1] - 200.0).abs() < 0.01);
        assert!((light.pixels[2] - 125.0).abs() < 0.01);
        assert!((light.pixels[3] - 250.0).abs() < 0.01);
    }

    #[test]
    fn test_calibrate_full() {
        use crate::{CalibrationMasters, StackingMethod};

        let mut light = AstroImage {
            metadata: AstroImageMetadata::default(),
            pixels: vec![115.0, 225.0, 170.0, 280.0],
            dimensions: ImageDimensions::new(2, 2, 1),
        };
        let bias = AstroImage {
            metadata: AstroImageMetadata::default(),
            pixels: vec![5.0, 5.0, 5.0, 5.0],
            dimensions: ImageDimensions::new(2, 2, 1),
        };
        let dark = AstroImage {
            metadata: AstroImageMetadata::default(),
            pixels: vec![10.0, 20.0, 15.0, 25.0],
            dimensions: ImageDimensions::new(2, 2, 1),
        };
        let flat = AstroImage {
            metadata: AstroImageMetadata::default(),
            pixels: vec![0.8, 1.0, 1.2, 1.0],
            dimensions: ImageDimensions::new(2, 2, 1),
        };

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
        assert!((light.pixels[0] - 125.0).abs() < 0.01);
        assert!((light.pixels[1] - 200.0).abs() < 0.01);
        assert!((light.pixels[2] - 125.0).abs() < 0.01);
        assert!((light.pixels[3] - 250.0).abs() < 0.01);
    }

    #[test]
    fn test_roundtrip_astro_to_image_to_astro() {
        let original = AstroImage {
            metadata: AstroImageMetadata::default(),
            pixels: vec![0.1, 0.2, 0.3, 0.4, 0.5, 0.6],
            dimensions: ImageDimensions::new(3, 2, 1),
        };

        // Convert to Image and back
        let image: Image = original.clone().into();
        let restored: AstroImage = image.into();

        // Verify dimensions preserved
        assert_eq!(restored.dimensions, original.dimensions);

        // Verify no stride padding
        assert_eq!(restored.pixels.len(), restored.dimensions.pixel_count());

        // Verify pixel values preserved
        for (a, b) in original.pixels.iter().zip(restored.pixels.iter()) {
            assert!((a - b).abs() < 1e-6, "Pixel mismatch: {} vs {}", a, b);
        }
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
            image.dimensions.width, image.dimensions.height, image.dimensions.channels
        );

        println!("Mean: {}", image.mean());

        assert!(image.dimensions.width > 0);
        assert!(image.dimensions.height > 0);
        // RGB images have 3 channels
        assert_eq!(image.dimensions.channels, 3);

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

        let original_dimensions = light.dimensions;

        println!(
            "Loaded light frame: {}x{}x{}",
            light.dimensions.width, light.dimensions.height, light.dimensions.channels
        );

        // Load calibration masters
        let Some(masters_dir) = calibration_masters_dir() else {
            eprintln!("calibration_masters directory not found, skipping test");
            return;
        };

        let start = std::time::Instant::now();
        let masters =
            CalibrationMasters::load_from_directory(&masters_dir, StackingMethod::default())
                .unwrap();
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
            light.dimensions.width, light.dimensions.height, light.dimensions.channels
        );

        println!("Mean: {}", light.mean());

        assert_eq!(light.dimensions, original_dimensions);

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
        let image = AstroImage {
            metadata: AstroImageMetadata::default(),
            // RGB pixels: R, G, B for each pixel, row-major order
            // Pixel (0,0): R=10, G=20, B=30
            // Pixel (1,0): R=40, G=50, B=60
            // Pixel (0,1): R=70, G=80, B=90
            // Pixel (1,1): R=100, G=110, B=120
            pixels: vec![
                10.0, 20.0, 30.0, 40.0, 50.0, 60.0, 70.0, 80.0, 90.0, 100.0, 110.0, 120.0,
            ],
            dimensions: ImageDimensions::new(2, 2, 3),
        };

        // Verify dimensions
        assert_eq!(image.dimensions.width, 2);
        assert_eq!(image.dimensions.height, 2);
        assert_eq!(image.dimensions.channels, 3);
        assert!(!image.dimensions.is_grayscale());
        // pixel_count() returns total values (width * height * channels)
        assert_eq!(image.pixel_count(), 12); // 2x2x3
        assert_eq!(image.dimensions.pixel_count(), 12);

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
        let mut light = AstroImage {
            metadata: AstroImageMetadata::default(),
            pixels: vec![
                100.0, 100.0, 100.0, // Pixel (0,0)
                200.0, 200.0, 200.0, // Pixel (1,0)
                150.0, 150.0, 150.0, // Pixel (0,1)
                250.0, 250.0, 250.0, // Pixel (1,1)
            ],
            dimensions: ImageDimensions::new(2, 2, 3),
        };

        // Create RGB bias frame (5.0 for all channels)
        let bias = AstroImage {
            metadata: AstroImageMetadata::default(),
            pixels: vec![5.0; 12],
            dimensions: ImageDimensions::new(2, 2, 3),
        };

        let masters = CalibrationMasters {
            master_dark: None,
            master_flat: None,
            master_bias: Some(bias),
            hot_pixel_map: None,
            method: StackingMethod::default(),
        };

        masters.calibrate(&mut light);

        // Each channel should have 5.0 subtracted
        assert_eq!(light.pixels[0], 95.0); // R of (0,0)
        assert_eq!(light.pixels[1], 95.0); // G of (0,0)
        assert_eq!(light.pixels[2], 95.0); // B of (0,0)
        assert_eq!(light.pixels[3], 195.0); // R of (1,0)
    }

    #[test]
    #[should_panic(expected = "don't match")]
    fn test_calibrate_rgb_with_grayscale_bias_panics() {
        use crate::{CalibrationMasters, StackingMethod};

        // RGB light frame
        let mut light = AstroImage {
            metadata: AstroImageMetadata::default(),
            pixels: vec![100.0; 12], // 2x2x3
            dimensions: ImageDimensions::new(2, 2, 3),
        };

        // Grayscale bias frame (channel mismatch!)
        let bias = AstroImage {
            metadata: AstroImageMetadata::default(),
            pixels: vec![5.0; 4], // 2x2x1
            dimensions: ImageDimensions::new(2, 2, 1),
        };

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
        let mut light = AstroImage {
            metadata: AstroImageMetadata::default(),
            pixels: vec![100.0; 4], // 2x2x1
            dimensions: ImageDimensions::new(2, 2, 1),
        };

        // RGB dark frame (channel mismatch!)
        let dark = AstroImage {
            metadata: AstroImageMetadata::default(),
            pixels: vec![10.0; 12], // 2x2x3
            dimensions: ImageDimensions::new(2, 2, 3),
        };

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
}
