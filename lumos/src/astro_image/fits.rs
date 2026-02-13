use anyhow::{Context, Result};
use fitsio::FitsFile;
use fitsio::hdu::HduInfo;
use fitsio::images::ImageType;
use std::path::Path;

use super::cfa::CfaType;
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

    let plane_size = img_dims.width * img_dims.height;
    assert_eq!(
        pixels.len(),
        plane_size * img_dims.channels,
        "Pixel count mismatch"
    );

    // Normalize integer FITS data to [0,1] range (RAW files are already normalized)
    let pixels = normalize_fits_pixels(pixels, bitpix);

    // Detect CFA pattern from BAYERPAT header
    let cfa_type = read_cfa_from_headers(&hdu, &mut fptr);

    // Read metadata
    let metadata = AstroImageMetadata {
        object: read_key_optional(&hdu, &mut fptr, "OBJECT"),
        instrument: read_key_optional(&hdu, &mut fptr, "INSTRUME"),
        telescope: read_key_optional(&hdu, &mut fptr, "TELESCOP"),
        date_obs: read_key_optional(&hdu, &mut fptr, "DATE-OBS"),
        exposure_time: read_key_optional(&hdu, &mut fptr, "EXPTIME"),
        iso: None,
        bitpix,
        header_dimensions: dimensions,
        cfa_type,
        filter: read_key_optional(&hdu, &mut fptr, "FILTER"),
        gain: read_key_optional(&hdu, &mut fptr, "GAIN"),
        egain: read_key_optional(&hdu, &mut fptr, "EGAIN"),
        ccd_temp: read_key_optional::<f64>(&hdu, &mut fptr, "CCD-TEMP")
            .or_else(|| read_key_optional(&hdu, &mut fptr, "CCDTEMP")),
        image_type: read_key_optional::<String>(&hdu, &mut fptr, "IMAGETYP")
            .or_else(|| read_key_optional(&hdu, &mut fptr, "FRAME")),
        xbinning: read_key_optional(&hdu, &mut fptr, "XBINNING"),
        ybinning: read_key_optional(&hdu, &mut fptr, "YBINNING"),
        set_temp: read_key_optional(&hdu, &mut fptr, "SET-TEMP"),
        offset: read_key_optional(&hdu, &mut fptr, "OFFSET"),
        focal_length: read_key_optional(&hdu, &mut fptr, "FOCALLEN"),
        airmass: read_key_optional(&hdu, &mut fptr, "AIRMASS"),
    };

    // FITS stores 3D images in planar order (all R, then all G, then all B).
    // Use from_planar_channels for RGB, from_pixels for grayscale.
    let mut astro = if img_dims.channels == 3 {
        let channels = pixels.chunks_exact(plane_size).map(|c| c.to_vec());
        AstroImage::from_planar_channels(img_dims, channels)
    } else {
        AstroImage::from_pixels(img_dims, pixels)
    };
    astro.metadata = metadata;
    Ok(astro)
}

/// Convert ImageType to BitPix enum.
///
/// cfitsio distinguishes signed from unsigned types (BZERO convention),
/// so we preserve this for correct normalization ranges.
fn image_type_to_bitpix(image_type: &ImageType) -> BitPix {
    match image_type {
        ImageType::UnsignedByte | ImageType::Byte => BitPix::UInt8,
        ImageType::Short => BitPix::Int16,
        ImageType::UnsignedShort => BitPix::UInt16,
        ImageType::Long => BitPix::Int32,
        ImageType::UnsignedLong => BitPix::UInt32,
        ImageType::LongLong => BitPix::Int64,
        ImageType::Float => BitPix::Float32,
        ImageType::Double => BitPix::Float64,
    }
}

/// Normalize integer FITS pixel data to [0,1] range.
///
/// cfitsio already applies BZERO/BSCALE, so values are in the correct integer
/// range. We divide by the max value for the BITPIX type. Float data is assumed
/// to already be in the correct range and is returned unchanged.
fn normalize_fits_pixels(mut pixels: Vec<f32>, bitpix: BitPix) -> Vec<f32> {
    if let Some(max_val) = bitpix.normalization_max() {
        let inv_max = 1.0 / max_val;
        pixels.iter_mut().for_each(|p| *p *= inv_max);
    }
    pixels
}

/// Read CFA pattern from BAYERPAT header, adjusting for ROWORDER if present.
///
/// BAYERPAT values: "RGGB", "BGGR", "GRBG", "GBRG", or "TRUE" (= RGGB).
/// ROWORDER: "TOP-DOWN" (default) or "BOTTOM-UP" (flips pattern vertically).
/// XBAYROFF/YBAYROFF: integer offsets into the Bayer matrix (shifts pattern).
fn read_cfa_from_headers(hdu: &fitsio::hdu::FitsHdu, fptr: &mut FitsFile) -> Option<CfaType> {
    use crate::raw::demosaic::CfaPattern;

    let bayerpat: String = read_key_optional(hdu, fptr, "BAYERPAT")?;
    let mut pattern = CfaPattern::from_bayerpat(&bayerpat)?;

    // ROWORDER: if BOTTOM-UP, the first row in memory is the bottom of the image,
    // so the Bayer pattern needs to be flipped vertically.
    if let Some(roworder) = read_key_optional::<String>(hdu, fptr, "ROWORDER")
        && roworder.trim().eq_ignore_ascii_case("BOTTOM-UP")
    {
        pattern = pattern.flip_vertical();
    }

    // XBAYROFF/YBAYROFF: offset into the Bayer matrix.
    // An odd Y offset flips rows, an odd X offset flips columns.
    let xoff: i32 = read_key_optional(hdu, fptr, "XBAYROFF").unwrap_or(0);
    let yoff: i32 = read_key_optional(hdu, fptr, "YBAYROFF").unwrap_or(0);
    if yoff & 1 != 0 {
        pattern = pattern.flip_vertical();
    }
    if xoff & 1 != 0 {
        pattern = pattern.flip_horizontal();
    }

    Some(CfaType::Bayer(pattern))
}

/// Helper to read an optional string key from FITS header.
fn read_key_optional<T: fitsio::headers::ReadsKey>(
    hdu: &fitsio::hdu::FitsHdu,
    fptr: &mut FitsFile,
    key: &str,
) -> Option<T> {
    hdu.read_key(fptr, key).ok()
}
