use anyhow::{Context, Result};
use fitsio::FitsFile;
use fitsio::hdu::HduInfo;
use fitsio::images::ImageType;
use std::path::Path;

use super::{AstroImage, AstroImageMetadata, BitPix, ImageDimensions};

/// Load an astronomical image from a FITS file.
pub fn load_fits(path: &Path) -> Result<AstroImage> {
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
    // FITS files don't indicate CFA status - assume false (processed data)
    let metadata = AstroImageMetadata {
        object: read_key_optional(&hdu, &mut fptr, "OBJECT"),
        instrument: read_key_optional(&hdu, &mut fptr, "INSTRUME"),
        telescope: read_key_optional(&hdu, &mut fptr, "TELESCOP"),
        date_obs: read_key_optional(&hdu, &mut fptr, "DATE-OBS"),
        exposure_time: read_key_optional(&hdu, &mut fptr, "EXPTIME"),
        iso: None, // FITS typically doesn't store ISO
        bitpix,
        header_dimensions: dimensions.clone(),
        is_cfa: false,
    };

    let mut astro = AstroImage::new(img_dims.width, img_dims.height, img_dims.channels, pixels);
    astro.metadata = metadata;
    Ok(astro)
}

/// Convert ImageType to BitPix enum.
fn image_type_to_bitpix(image_type: &ImageType) -> BitPix {
    match image_type {
        ImageType::UnsignedByte | ImageType::Byte => BitPix::UInt8,
        ImageType::Short | ImageType::UnsignedShort => BitPix::Int16,
        ImageType::Long | ImageType::UnsignedLong => BitPix::Int32,
        ImageType::LongLong => BitPix::Int64,
        ImageType::Float => BitPix::Float32,
        ImageType::Double => BitPix::Float64,
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
