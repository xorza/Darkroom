use anyhow::{Context, Result};
use fitsio::FitsFile;
use fitsio::hdu::HduInfo;
use fitsio::images::ImageType;
use std::path::Path;

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
    /// Image dimensions [width, height] or [width, height, channels]
    pub dimensions: Vec<usize>,
}

/// Represents an astronomical image loaded from a FITS file.
#[derive(Debug)]
pub struct AstroImage {
    /// Image metadata from FITS headers
    pub metadata: AstroImageMetadata,
    /// Pixel data stored as f32 for processing flexibility
    pub pixels: Vec<f32>,
    /// Image width in pixels
    pub width: usize,
    /// Image height in pixels
    pub height: usize,
    /// Number of channels (1 for grayscale, 3 for RGB)
    pub channels: usize,
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
        let (width, height, channels) = match dimensions.len() {
            2 => (dimensions[1], dimensions[0], 1),
            3 => (dimensions[2], dimensions[1], dimensions[0]),
            n => anyhow::bail!("Unsupported number of dimensions: {}", n),
        };

        assert!(width > 0, "Image width must be positive");
        assert!(height > 0, "Image height must be positive");

        // Read pixel data as f32
        let pixels: Vec<f32> = hdu
            .read_image(&mut fptr)
            .context("Failed to read image data")?;

        let expected_size = width * height * channels;
        assert!(
            pixels.len() == expected_size,
            "Pixel count mismatch: expected {}, got {}",
            expected_size,
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
            dimensions: dimensions.clone(),
        };

        Ok(AstroImage {
            metadata,
            pixels,
            width,
            height,
            channels,
        })
    }

    /// Get pixel value at (x, y) for single-channel images.
    /// Panics if coordinates are out of bounds.
    pub fn get_pixel_gray(&self, x: usize, y: usize) -> f32 {
        debug_assert!(x < self.width, "x coordinate out of bounds");
        debug_assert!(y < self.height, "y coordinate out of bounds");
        debug_assert_eq!(
            self.channels, 1,
            "Use get_pixel_rgb for multi-channel images"
        );

        self.pixels[y * self.width + x]
    }

    /// Get pixel values at (x, y) for multi-channel images.
    /// Returns a slice of channel values.
    /// Panics if coordinates are out of bounds.
    pub fn get_pixel_rgb(&self, x: usize, y: usize) -> &[f32; 3] {
        debug_assert!(x < self.width, "x coordinate out of bounds");
        debug_assert!(y < self.height, "y coordinate out of bounds");
        debug_assert_eq!(self.channels, 3, "Image must have at least 3 channels");

        let idx = (y * self.width + x) * self.channels;
        self.pixels[idx..idx + 3].as_array::<3>().unwrap()
    }

    /// Get mutable reference to pixel value at (x, y) for single-channel images.
    /// Panics if coordinates are out of bounds.
    pub fn get_pixel_gray_mut(&mut self, x: usize, y: usize) -> &mut f32 {
        debug_assert!(x < self.width, "x coordinate out of bounds");
        debug_assert!(y < self.height, "y coordinate out of bounds");
        debug_assert_eq!(
            self.channels, 1,
            "Use get_pixel_rgb_mut for multi-channel images"
        );

        let idx = y * self.width + x;
        &mut self.pixels[idx]
    }

    /// Get mutable reference to pixel values at (x, y) for multi-channel images.
    /// Returns a mutable slice of channel values.
    /// Panics if coordinates are out of bounds.
    pub fn get_pixel_rgb_mut(&mut self, x: usize, y: usize) -> &mut [f32; 3] {
        debug_assert!(x < self.width, "x coordinate out of bounds");
        debug_assert!(y < self.height, "y coordinate out of bounds");
        debug_assert_eq!(self.channels, 3, "Image must have at least 3 channels");

        let idx = (y * self.width + x) * self.channels;
        (&mut self.pixels[idx..idx + 3]).try_into().unwrap()
    }

    /// Get the total number of pixels (width * height * channels).
    pub fn pixel_count(&self) -> usize {
        self.pixels.len()
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

    #[test]
    fn test_metadata_default() {
        let meta = AstroImageMetadata::default();
        assert!(meta.object.is_none());
        assert!(meta.dimensions.is_empty());
    }

    #[test]
    fn test_load_full_example_fits() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../test_resources/full_example.fits"
        );
        let image = AstroImage::from_fits(path).unwrap();

        assert_eq!(image.width, 100);
        assert_eq!(image.height, 100);
        assert_eq!(image.channels, 1);
        assert_eq!(image.pixel_count(), 10000);
        assert_eq!(image.metadata.bitpix, 32);
        assert_eq!(image.metadata.dimensions, vec![100, 100]);

        // Test pixel access
        let pixel = image.get_pixel_gray(5, 20);
        assert_eq!(pixel, 152.0);
    }
}
