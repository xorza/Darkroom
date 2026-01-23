mod fits;
mod libraw;
mod rawloader;

use anyhow::Result;
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
    /// Load an astronomical image from a FITS file.
    ///
    /// # Arguments
    /// * `path` - Path to the FITS file
    ///
    /// # Returns
    /// * `Result<AstroImage>` - The loaded image or an error
    pub fn from_fits<P: AsRef<Path>>(path: P) -> Result<Self> {
        fits::load_fits(path.as_ref())
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

    /// Load an astronomical image from a raw camera file (RAF, CR2, NEF, etc.).
    ///
    /// Returns the raw Bayer mosaic data as a single-channel image.
    /// The data is normalized to 0.0-1.0 range.
    ///
    /// Tries rawloader first (faster, pure Rust), falls back to libraw if unsupported.
    ///
    /// # Arguments
    /// * `path` - Path to the raw file
    ///
    /// # Returns
    /// * `Result<AstroImage>` - The loaded image or an error
    pub fn from_raw<P: AsRef<Path>>(path: P) -> Result<Self> {
        let path = path.as_ref();

        // Try rawloader first (faster, pure Rust)
        if let Ok(image) = rawloader::load_raw(path) {
            return Ok(image);
        }

        // Fall back to libraw for unsupported cameras
        libraw::load_raw(path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::env;
    use std::fs;

    #[test]
    fn test_metadata_default() {
        let meta = AstroImageMetadata::default();
        assert!(meta.object.is_none());
        assert!(meta.header_dimensions.is_empty());
    }

    #[test]
    fn test_load_full_example_fits() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../test_resources/full_example.fits"
        );
        let image = AstroImage::from_fits(path).unwrap();

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
    fn test_load_single_raw_from_env() {
        let Ok(calibration_dir) = env::var("LUMOS_CALIBRATION_DIR") else {
            eprintln!("LUMOS_CALIBRATION_DIR not set, skipping test");
            return;
        };

        let darks_dir = std::path::Path::new(&calibration_dir).join("Darks");
        assert!(
            darks_dir.exists(),
            "Darks directory does not exist: {}",
            darks_dir.display()
        );

        // Find first raw file
        let first_raw = fs::read_dir(&darks_dir)
            .expect("Failed to read Darks directory")
            .filter_map(|e| e.ok())
            .find(|e| {
                let path = e.path();
                if !path.is_file() {
                    return false;
                }
                let ext = path.extension().and_then(|s| s.to_str()).unwrap_or("");
                matches!(
                    ext.to_lowercase().as_str(),
                    "raf" | "cr2" | "cr3" | "nef" | "arw" | "dng"
                )
            });

        let Some(entry) = first_raw else {
            eprintln!("No raw files found in Darks directory, skipping test");
            return;
        };

        let path = entry.path();
        println!("Loading raw file: {}", path.display());

        let image = AstroImage::from_raw(&path).expect("Failed to load raw file");

        println!(
            "Loaded: {} ({}x{}x{})",
            path.file_name().unwrap().to_string_lossy(),
            image.dimensions.width,
            image.dimensions.height,
            image.dimensions.channels
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
