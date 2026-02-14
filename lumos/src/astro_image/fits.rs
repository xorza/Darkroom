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
        iso: read_key_optional::<i32>(&hdu, &mut fptr, "ISOSPEED").map(|v| v as u32),
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
        ra_deg: read_ra_deg(&hdu, &mut fptr),
        dec_deg: read_dec_deg(&hdu, &mut fptr),
        pixel_size_x: read_key_optional(&hdu, &mut fptr, "XPIXSZ"),
        pixel_size_y: read_key_optional(&hdu, &mut fptr, "YPIXSZ"),
        data_max: read_key_optional(&hdu, &mut fptr, "DATAMAX"),
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

/// Normalize FITS pixel data to [0,1] range.
///
/// Integer types: cfitsio already applies BZERO/BSCALE, so values are in the
/// correct integer range. We divide by the max value for the BITPIX type.
///
/// Float types: the FITS standard does not define a normalization convention.
/// Data may be in [0,1] (PixInsight), [0,65535] (DeepSkyStacker), [0,255],
/// or arbitrary physical units. We detect the actual max value and normalize
/// if it exceeds 2.0 (threshold provides headroom for HDR/overexposed pixels
/// while catching the common integer-like ranges).
fn normalize_fits_pixels(mut pixels: Vec<f32>, bitpix: BitPix) -> Vec<f32> {
    let inv_max = if let Some(max_val) = bitpix.normalization_max() {
        1.0 / max_val
    } else {
        // Float types: sanitize NaN/Inf to 0.0 and find max in one pass
        let mut max = f32::NEG_INFINITY;
        for p in pixels.iter_mut() {
            if p.is_finite() {
                max = max.max(*p);
            } else {
                *p = 0.0;
            }
        }
        if max > 2.0 { 1.0 / max } else { return pixels }
    };
    pixels.iter_mut().for_each(|p| *p *= inv_max);
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

/// Read RA in degrees from FITS headers.
///
/// Tries: `RA` (degrees, NINA/SGP), `OBJCTRA` (HMS string, MaximDL/ASCOM),
/// `CRVAL1` (WCS reference point, plate-solved images).
fn read_ra_deg(hdu: &fitsio::hdu::FitsHdu, fptr: &mut FitsFile) -> Option<f64> {
    if let Some(ra) = read_key_optional::<f64>(hdu, fptr, "RA") {
        return Some(ra);
    }
    if let Some(s) = read_key_optional::<String>(hdu, fptr, "OBJCTRA") {
        return parse_hms_to_deg(&s);
    }
    read_key_optional::<f64>(hdu, fptr, "CRVAL1")
}

/// Read DEC in degrees from FITS headers.
///
/// Tries: `DEC` (degrees), `OBJCTDEC` (DMS string), `CRVAL2` (WCS).
fn read_dec_deg(hdu: &fitsio::hdu::FitsHdu, fptr: &mut FitsFile) -> Option<f64> {
    if let Some(dec) = read_key_optional::<f64>(hdu, fptr, "DEC") {
        return Some(dec);
    }
    if let Some(s) = read_key_optional::<String>(hdu, fptr, "OBJCTDEC") {
        return parse_dms_to_deg(&s);
    }
    read_key_optional::<f64>(hdu, fptr, "CRVAL2")
}

/// Parse HMS string "HH MM SS.ss" to degrees.
/// Accepts both space-delimited and colon-delimited formats.
fn parse_hms_to_deg(s: &str) -> Option<f64> {
    let parts: Vec<f64> = s
        .split([' ', ':'])
        .filter(|p| !p.is_empty())
        .map(|p| p.trim().parse().ok())
        .collect::<Option<Vec<_>>>()?;
    if parts.len() != 3 {
        return None;
    }
    // RA in hours: deg = (h + m/60 + s/3600) * 15
    let sign = if parts[0].is_sign_negative() {
        -1.0
    } else {
        1.0
    };
    Some(sign * (parts[0].abs() + parts[1] / 60.0 + parts[2] / 3600.0) * 15.0)
}

/// Parse DMS string "±DD MM SS.ss" to degrees.
/// Accepts both space-delimited and colon-delimited formats.
fn parse_dms_to_deg(s: &str) -> Option<f64> {
    let parts: Vec<f64> = s
        .split([' ', ':'])
        .filter(|p| !p.is_empty())
        .map(|p| p.trim().parse().ok())
        .collect::<Option<Vec<_>>>()?;
    if parts.len() != 3 {
        return None;
    }
    let sign = if parts[0].is_sign_negative() {
        -1.0
    } else {
        1.0
    };
    Some(sign * (parts[0].abs() + parts[1] / 60.0 + parts[2] / 3600.0))
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

    // ====================================================================
    // normalize_fits_pixels tests
    // ====================================================================

    #[test]
    fn test_normalize_float_already_01() {
        // Pixels already in [0, 1] — should be unchanged
        let pixels = vec![0.0, 0.25, 0.5, 0.75, 1.0];
        let result = normalize_fits_pixels(pixels.clone(), BitPix::Float32);
        assert_eq!(result, pixels);
    }

    #[test]
    fn test_normalize_float_hdr_headroom() {
        // max = 1.5 (HDR overexposure) — below threshold 2.0, unchanged
        let pixels = vec![0.0, 0.5, 1.0, 1.5];
        let result = normalize_fits_pixels(pixels.clone(), BitPix::Float32);
        assert_eq!(result, pixels);
    }

    #[test]
    fn test_normalize_float_at_threshold() {
        // max = 2.0 — at threshold, should NOT normalize (threshold is > 2.0, not >=)
        let pixels = vec![0.0, 1.0, 2.0];
        let result = normalize_fits_pixels(pixels.clone(), BitPix::Float32);
        assert_eq!(result, pixels);
    }

    #[test]
    fn test_normalize_float_0_65535_range() {
        // DeepSkyStacker output: [0, 65535] — should normalize by dividing by max
        // inv_max = 1.0 / 65535.0
        let pixels = vec![0.0, 32767.5, 65535.0];
        let result = normalize_fits_pixels(pixels, BitPix::Float32);
        // 0.0 / 65535.0 = 0.0
        assert!((result[0] - 0.0).abs() < 1e-7);
        // 32767.5 / 65535.0 = 0.5
        assert!((result[1] - 0.5).abs() < 1e-4);
        // 65535.0 / 65535.0 = 1.0
        assert!((result[2] - 1.0).abs() < 1e-7);
    }

    #[test]
    fn test_normalize_float_0_255_range() {
        // 8-bit style float: [0, 255] — should normalize
        // inv_max = 1.0 / 255.0
        let pixels = vec![0.0, 127.5, 255.0];
        let result = normalize_fits_pixels(pixels, BitPix::Float32);
        // 0.0 / 255.0 = 0.0
        assert!((result[0] - 0.0).abs() < 1e-7);
        // 127.5 / 255.0 = 0.5
        assert!((result[1] - 0.5).abs() < 1e-4);
        // 255.0 / 255.0 = 1.0
        assert!((result[2] - 1.0).abs() < 1e-7);
    }

    #[test]
    fn test_normalize_float_negative_values_preserved() {
        // Dark-subtracted data can have negative values.
        // max = 100.0 > 2.0 → normalize by max. Negatives scale proportionally.
        let pixels = vec![-5.0, 0.0, 50.0, 100.0];
        let result = normalize_fits_pixels(pixels, BitPix::Float32);
        // inv_max = 1.0 / 100.0 = 0.01
        // -5.0 * 0.01 = -0.05
        assert!((result[0] - (-0.05)).abs() < 1e-7);
        // 0.0 * 0.01 = 0.0
        assert!((result[1] - 0.0).abs() < 1e-7);
        // 50.0 * 0.01 = 0.5
        assert!((result[2] - 0.5).abs() < 1e-7);
        // 100.0 * 0.01 = 1.0
        assert!((result[3] - 1.0).abs() < 1e-7);
    }

    #[test]
    fn test_normalize_float64_same_behavior() {
        // Float64 (BITPIX -64) should behave identically to Float32
        let pixels = vec![0.0, 32767.5, 65535.0];
        let result = normalize_fits_pixels(pixels, BitPix::Float64);
        assert!((result[0] - 0.0).abs() < 1e-7);
        assert!((result[1] - 0.5).abs() < 1e-4);
        assert!((result[2] - 1.0).abs() < 1e-7);
    }

    #[test]
    fn test_normalize_uint16_unchanged() {
        // Integer types still use fixed normalization_max, not heuristic
        // UInt16: max = 65535.0
        let pixels = vec![0.0, 32767.5, 65535.0];
        let result = normalize_fits_pixels(pixels, BitPix::UInt16);
        // 0.0 / 65535.0 = 0.0
        assert!((result[0] - 0.0).abs() < 1e-7);
        // 32767.5 / 65535.0 ≈ 0.49999
        assert!((result[1] - 32767.5 / 65535.0).abs() < 1e-7);
        // 65535.0 / 65535.0 = 1.0
        assert!((result[2] - 1.0).abs() < 1e-7);
    }

    #[test]
    fn test_normalize_float_all_zero() {
        // All zeros: max = 0.0, which is <= 2.0, so unchanged
        let pixels = vec![0.0, 0.0, 0.0];
        let result = normalize_fits_pixels(pixels.clone(), BitPix::Float32);
        assert_eq!(result, pixels);
    }

    #[test]
    fn test_normalize_float_single_pixel() {
        // Single pixel at 50000 → normalizes to 1.0
        let pixels = vec![50000.0];
        let result = normalize_fits_pixels(pixels, BitPix::Float32);
        assert!((result[0] - 1.0).abs() < 1e-7);
    }

    // ====================================================================
    // NaN/Inf sanitization tests (float FITS only)
    // ====================================================================

    #[test]
    fn test_normalize_float_nan_replaced_with_zero() {
        // FITS uses NaN as null indicator for float images.
        // NaN pixels should become 0.0. Valid pixels unchanged (max=0.8 < 2.0).
        let pixels = vec![0.0, f32::NAN, 0.5, f32::NAN, 0.8];
        let result = normalize_fits_pixels(pixels, BitPix::Float32);
        assert_eq!(result, vec![0.0, 0.0, 0.5, 0.0, 0.8]);
    }

    #[test]
    fn test_normalize_float_inf_replaced_with_zero() {
        // +Inf and -Inf should become 0.0. Valid pixels unchanged (max=1.0 < 2.0).
        let pixels = vec![0.5, f32::INFINITY, 1.0, f32::NEG_INFINITY];
        let result = normalize_fits_pixels(pixels, BitPix::Float32);
        assert_eq!(result, vec![0.5, 0.0, 1.0, 0.0]);
    }

    #[test]
    fn test_normalize_float_nan_with_normalization() {
        // NaN replaced before normalization. max of valid pixels = 100.0 > 2.0.
        // inv_max = 1/100 = 0.01
        // [NaN→0.0, 50.0, 100.0] → [0.0*0.01, 50.0*0.01, 100.0*0.01] = [0.0, 0.5, 1.0]
        let pixels = vec![f32::NAN, 50.0, 100.0];
        let result = normalize_fits_pixels(pixels, BitPix::Float32);
        assert!((result[0] - 0.0).abs() < 1e-7);
        assert!((result[1] - 0.5).abs() < 1e-7);
        assert!((result[2] - 1.0).abs() < 1e-7);
    }

    #[test]
    fn test_normalize_float_all_nan() {
        // All NaN → all become 0.0. max of remaining = 0.0 < 2.0, no normalization.
        let pixels = vec![f32::NAN, f32::NAN, f32::NAN];
        let result = normalize_fits_pixels(pixels, BitPix::Float32);
        assert_eq!(result, vec![0.0, 0.0, 0.0]);
    }

    #[test]
    fn test_normalize_float64_nan_replaced() {
        // Float64 follows same path as Float32
        let pixels = vec![0.0, f32::NAN, 0.5];
        let result = normalize_fits_pixels(pixels, BitPix::Float64);
        assert_eq!(result, vec![0.0, 0.0, 0.5]);
    }

    #[test]
    fn test_normalize_uint16_nan_not_sanitized() {
        // Integer types use fixed normalization_max, NaN handling is float-only.
        // NaN * inv_max = NaN — integer FITS should never contain NaN from cfitsio,
        // so we don't sanitize (would mask a real bug).
        let pixels = vec![0.0, f32::NAN, 65535.0];
        let result = normalize_fits_pixels(pixels, BitPix::UInt16);
        assert!((result[0] - 0.0).abs() < 1e-7);
        assert!(result[1].is_nan()); // Not sanitized for integer types
        assert!((result[2] - 1.0).abs() < 1e-7);
    }

    // ====================================================================
    // RA/DEC coordinate parsing tests
    // ====================================================================

    #[test]
    fn test_parse_hms_space_delimited() {
        // M42: RA = 05h 35m 17.3s = (5 + 35/60 + 17.3/3600) * 15 = 83.82208333... deg
        let deg = parse_hms_to_deg("05 35 17.3").unwrap();
        let expected = (5.0 + 35.0 / 60.0 + 17.3 / 3600.0) * 15.0; // 83.82208333
        assert!(
            (deg - expected).abs() < 1e-10,
            "got {deg}, expected {expected}"
        );
    }

    #[test]
    fn test_parse_hms_colon_delimited() {
        // Same value with colons
        let deg = parse_hms_to_deg("05:35:17.3").unwrap();
        let expected = (5.0 + 35.0 / 60.0 + 17.3 / 3600.0) * 15.0;
        assert!((deg - expected).abs() < 1e-10);
    }

    #[test]
    fn test_parse_hms_zero() {
        // RA = 00h 00m 00.0s = 0.0 deg
        let deg = parse_hms_to_deg("00 00 00.0").unwrap();
        assert!((deg - 0.0).abs() < 1e-10);
    }

    #[test]
    fn test_parse_dms_positive() {
        // M42 DEC: -05° 23' 28.0" = -(5 + 23/60 + 28/3600) = -5.39111...
        let deg = parse_dms_to_deg("-05 23 28.0").unwrap();
        let expected = -(5.0 + 23.0 / 60.0 + 28.0 / 3600.0); // -5.39111
        assert!(
            (deg - expected).abs() < 1e-10,
            "got {deg}, expected {expected}"
        );
    }

    #[test]
    fn test_parse_dms_colon_delimited() {
        let deg = parse_dms_to_deg("+45:30:15.5").unwrap();
        let expected = 45.0 + 30.0 / 60.0 + 15.5 / 3600.0; // 45.50430555
        assert!((deg - expected).abs() < 1e-10);
    }

    #[test]
    fn test_parse_dms_negative_zero_degrees() {
        // DEC = -00° 30' 00.0" = -0.5 deg (sign on zero degrees)
        let deg = parse_dms_to_deg("-00 30 00.0").unwrap();
        let expected = -0.5;
        assert!(
            (deg - expected).abs() < 1e-10,
            "got {deg}, expected {expected}"
        );
    }

    #[test]
    fn test_parse_hms_invalid_parts() {
        assert!(parse_hms_to_deg("05 35").is_none()); // only 2 parts
        assert!(parse_hms_to_deg("").is_none());
        assert!(parse_hms_to_deg("abc def ghi").is_none());
    }

    #[test]
    fn test_parse_dms_invalid_parts() {
        assert!(parse_dms_to_deg("45 30").is_none());
        assert!(parse_dms_to_deg("").is_none());
    }
}
