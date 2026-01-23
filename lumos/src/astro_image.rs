use anyhow::{Context, Result};
use fitsio::FitsFile;
use fitsio::hdu::HduInfo;
use fitsio::images::ImageType;
use rawloader::RawImageData;
use std::path::Path;

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
    /// Bits per pixel (BITPIX keyword)
    pub bitpix: i32,
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
        let path = path.as_ref();
        let mut fptr = FitsFile::open(path)
            .with_context(|| format!("Failed to open FITS file: {}", path.display()))?;

        let hdu = fptr.primary_hdu().context("Failed to access primary HDU")?;

        let (dimensions, bitpix) = match &hdu.info {
            HduInfo::ImageInfo { shape, image_type } => {
                let bitpix = image_type_to_bitpix(image_type);
                (shape.clone(), bitpix)
            }
            HduInfo::TableInfo { .. } => {
                anyhow::bail!("Primary HDU is a table, not an image");
            }
            HduInfo::AnyInfo => {
                anyhow::bail!("Unknown HDU type");
            }
        };

        assert!(
            !dimensions.is_empty(),
            "Image must have at least one dimension"
        );

        // FITS dimensions are in NAXIS order: NAXIS1 (width), NAXIS2 (height), NAXIS3 (channels)
        // But shape is returned in reverse order: [channels, height, width] or [height, width]
        let img_dims = match dimensions.len() {
            2 => ImageDimensions::new(dimensions[1], dimensions[0], 1),
            3 => ImageDimensions::new(dimensions[2], dimensions[1], dimensions[0]),
            n => anyhow::bail!("Unsupported number of dimensions: {}", n),
        };

        // Read pixel data as f32
        let pixels: Vec<f32> = hdu
            .read_image(&mut fptr)
            .context("Failed to read image data")?;

        assert!(
            pixels.len() == img_dims.pixel_count(),
            "Pixel count mismatch: expected {}, got {}",
            img_dims.pixel_count(),
            pixels.len()
        );

        // Read metadata
        let metadata = AstroImageMetadata {
            object: read_key_optional(&hdu, &mut fptr, "OBJECT"),
            instrument: read_key_optional(&hdu, &mut fptr, "INSTRUME"),
            telescope: read_key_optional(&hdu, &mut fptr, "TELESCOP"),
            date_obs: read_key_optional(&hdu, &mut fptr, "DATE-OBS"),
            exposure_time: read_key_optional(&hdu, &mut fptr, "EXPTIME"),
            bitpix,
            header_dimensions: dimensions.clone(),
        };

        Ok(AstroImage {
            metadata,
            pixels,
            dimensions: img_dims,
        })
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
    /// The data is normalized to 0.0-1.0 range based on black/white levels.
    ///
    /// # Arguments
    /// * `path` - Path to the raw file
    ///
    /// # Returns
    /// * `Result<AstroImage>` - The loaded image or an error
    pub fn from_raw<P: AsRef<Path>>(path: P) -> Result<Self> {
        let path = path.as_ref();
        let raw_image = rawloader::decode_file(path)
            .with_context(|| format!("Failed to decode raw file: {}", path.display()))?;

        let width = raw_image.width;
        let height = raw_image.height;
        let cpp = raw_image.cpp; // components per pixel (1 for Bayer, 3 for RGB sensors)

        let dimensions = ImageDimensions::new(width, height, cpp);

        // Get black and white levels for normalization
        let black_level = raw_image.blacklevels[0] as f32;
        let white_level = raw_image.whitelevels[0] as f32;
        let range = white_level - black_level;

        let is_float = matches!(&raw_image.data, RawImageData::Float(_));
        let pixels: Vec<f32> = match raw_image.data {
            RawImageData::Integer(data) => data
                .iter()
                .map(|&v| ((v as f32) - black_level) / range)
                .collect(),
            RawImageData::Float(data) => data.iter().map(|&v| (v - black_level) / range).collect(),
        };

        assert!(
            pixels.len() == dimensions.pixel_count(),
            "Pixel count mismatch: expected {}, got {}",
            dimensions.pixel_count(),
            pixels.len()
        );

        let metadata = AstroImageMetadata {
            object: None,
            instrument: Some(format!(
                "{} {}",
                raw_image.clean_make, raw_image.clean_model
            )),
            telescope: None,
            date_obs: None,
            exposure_time: None,
            bitpix: if is_float { -32 } else { 16 },
            header_dimensions: vec![height, width],
        };

        Ok(AstroImage {
            metadata,
            pixels,
            dimensions,
        })
    }
}

/// Convert ImageType to BITPIX value.
fn image_type_to_bitpix(image_type: &ImageType) -> i32 {
    match image_type {
        ImageType::UnsignedByte | ImageType::Byte => 8,
        ImageType::Short | ImageType::UnsignedShort => 16,
        ImageType::Long | ImageType::UnsignedLong => 32,
        ImageType::LongLong => 64,
        ImageType::Float => -32,
        ImageType::Double => -64,
    }
}

/// Helper to read an optional string key from FITS header.
fn read_key_optional<T: fitsio::headers::ReadsKey>(
    hdu: &fitsio::hdu::FitsHdu,
    fptr: &mut FitsFile,
    key: &str,
) -> Option<T> {
    hdu.read_key(fptr, key).ok()
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
        assert_eq!(image.metadata.bitpix, 32);
        assert_eq!(image.metadata.header_dimensions, vec![100, 100]);

        // Test pixel access
        let pixel = image.get_pixel_gray(5, 20);
        assert_eq!(pixel, 152.0);
    }

    #[test]
    fn test_load_darks_from_env() {
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

        let entries: Vec<_> = fs::read_dir(&darks_dir)
            .expect("Failed to read Darks directory")
            .filter_map(|e| e.ok())
            .filter(|e| {
                let path = e.path();
                if !path.is_file() {
                    return false;
                }
                let ext = path.extension().and_then(|s| s.to_str()).unwrap_or("");
                matches!(
                    ext.to_lowercase().as_str(),
                    "fit" | "fits" | "raf" | "cr2" | "cr3" | "nef" | "arw" | "dng"
                )
            })
            .collect();

        assert!(
            !entries.is_empty(),
            "No image files found in Darks directory"
        );

        println!(
            "Found {} dark frames in {}",
            entries.len(),
            darks_dir.display()
        );

        let mut images = Vec::new();
        let mut first_dims: Option<ImageDimensions> = None;

        for entry in &entries {
            let path = entry.path();
            let ext = path.extension().and_then(|s| s.to_str()).unwrap_or("");

            let image = if matches!(ext.to_lowercase().as_str(), "fit" | "fits") {
                AstroImage::from_fits(&path)
            } else {
                AstroImage::from_raw(&path)
            };

            match image {
                Ok(img) => {
                    println!(
                        "Loaded: {} ({}x{}x{})",
                        path.file_name().unwrap().to_string_lossy(),
                        img.dimensions.width,
                        img.dimensions.height,
                        img.dimensions.channels
                    );

                    // Verify all images have same dimensions
                    if let Some(dims) = first_dims {
                        assert_eq!(
                            img.dimensions,
                            dims,
                            "Dimension mismatch in {}",
                            path.display()
                        );
                    } else {
                        first_dims = Some(img.dimensions);
                    }

                    images.push(img);
                }
                Err(e) => {
                    eprintln!("Failed to load {}: {}", path.display(), e);
                }
            }
        }

        assert!(
            !images.is_empty(),
            "Failed to load any images from Darks directory"
        );
        println!("Successfully loaded {} dark frames", images.len());
    }
}
