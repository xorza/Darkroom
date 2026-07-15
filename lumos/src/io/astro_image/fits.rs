use std::path::Path;

use fits_well::{FitsReader, Header, SampleType};
use rayon::prelude::*;

use crate::io::astro_image::cfa::CfaType;
use crate::io::astro_image::error::ImageError;
use crate::io::astro_image::{AstroImage, AstroImageMetadata, BitPix, ImageDimensions};

fn fits_err(path: &Path, source: fits_well::FitsError) -> ImageError {
    ImageError::Fits {
        path: path.to_path_buf(),
        source,
    }
}

fn fits_unsupported(path: &Path, reason: impl Into<String>) -> ImageError {
    ImageError::FitsUnsupported {
        path: path.to_path_buf(),
        reason: reason.into(),
    }
}

/// Load an astronomical image from a FITS file.
pub(crate) fn load_fits(path: &Path) -> Result<AstroImage, ImageError> {
    // Read the whole file once, then decode straight from the bytes: `from_bytes`
    // borrows the buffer in place, so `read_image` skips the per-data-unit staging
    // copy that the seeking `open` path pays.
    let bytes = std::fs::read(path).map_err(|source| ImageError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    let mut reader = FitsReader::from_bytes(&bytes).map_err(|e| fits_err(path, e))?;

    // The first image-bearing HDU: skips an empty (NAXIS=0) primary and reads a
    // tile-compressed image transparently, which `primary_hdu` could not do.
    let index = *reader
        .image_indices()
        .first()
        .ok_or_else(|| fits_unsupported(path, "no image HDU found"))?;

    // Decode pixels and shape, then drop the data-unit borrow before reading headers.
    let (shape, bitpix, pixels) = {
        let raw = reader.read_image(index).map_err(|e| fits_err(path, e))?;
        let bitpix = map_bitpix(raw.sample_type());
        // `physical_f32` applies BSCALE/BZERO and maps the integer BLANK to NaN (the
        // effective values cfitsio's f32 read produced), narrowing in a single pass.
        let pixels: Vec<f32> = raw.physical_f32();
        (raw.shape.clone(), bitpix, pixels)
    };

    // fits-well reports shape NAXIS1-first: [width, height, (channels)]. All shapes other than
    // 2D or 3D-with-NAXIS3∈{1,3} are rejected as `Err` — never asserted — because `shape` comes
    // from untrusted file input (the `n` arm also covers the 0/1-dimension cases).
    let img_dims = match shape.len() {
        2 => ImageDimensions::new((shape[0], shape[1]), 1),
        3 if shape[2] == 1 || shape[2] == 3 => ImageDimensions::new((shape[0], shape[1]), shape[2]),
        3 => {
            return Err(fits_unsupported(
                path,
                format!("Unsupported channel count (NAXIS3): {}", shape[2]),
            ));
        }
        n => {
            return Err(fits_unsupported(
                path,
                format!("Unsupported number of dimensions: {n}"),
            ));
        }
    };

    // Shape↔buffer mismatch means a corrupt/unsupported file, not a logic error — return `Err`.
    let plane_size = img_dims.size.x * img_dims.size.y;
    if pixels.len() != plane_size * img_dims.channels {
        return Err(fits_unsupported(
            path,
            format!("pixel count {} doesn't match shape {shape:?}", pixels.len()),
        ));
    }

    let header = &reader.hdus()[index].header;

    // DATAMAX (the writer's declared full-scale) is the frame-independent normalization divisor
    // when present; see `normalize_fits_pixels`. RAW files arrive already normalized.
    let data_max = header.get_real("DATAMAX");
    let pixels = normalize_fits_pixels(pixels, bitpix, data_max);

    // Detect CFA pattern from BAYERPAT header
    let cfa_type = read_cfa_from_headers(header);

    // Read metadata
    let metadata = AstroImageMetadata {
        object: read_text(header, "OBJECT"),
        instrument: read_text(header, "INSTRUME"),
        telescope: read_text(header, "TELESCOP"),
        date_obs: read_text(header, "DATE-OBS"),
        exposure_time: header.get_real("EXPTIME"),
        iso: header.get_integer("ISOSPEED").map(|v| v as u32),
        bitpix,
        header_dimensions: shape,
        cfa_type,
        filter: read_text(header, "FILTER"),
        gain: header.get_real("GAIN"),
        egain: header.get_real("EGAIN"),
        ccd_temp: header
            .get_real("CCD-TEMP")
            .or_else(|| header.get_real("CCDTEMP")),
        image_type: read_text(header, "IMAGETYP").or_else(|| read_text(header, "FRAME")),
        xbinning: header.get_integer("XBINNING").map(|v| v as i32),
        ybinning: header.get_integer("YBINNING").map(|v| v as i32),
        set_temp: header.get_real("SET-TEMP"),
        offset: header.get_integer("OFFSET").map(|v| v as i32),
        focal_length: header.get_real("FOCALLEN"),
        airmass: header.get_real("AIRMASS"),
        ra_deg: read_ra_deg(header),
        dec_deg: read_dec_deg(header),
        pixel_size_x: header.get_real("XPIXSZ"),
        pixel_size_y: header.get_real("YPIXSZ"),
        data_max,
        calibrated: false,
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

/// Map fits-well's effective `SampleType` to lumos's `BitPix`.
///
/// `SampleType` already resolves the FITS unsigned/signed-byte BZERO conventions, so
/// the normalization range falls straight out. lumos has no signed-byte or unsigned-64
/// type, so `I8` folds into `UInt8` and `U64` into `Int64` (as the cfitsio path did).
fn map_bitpix(sample_type: SampleType) -> BitPix {
    match sample_type {
        SampleType::I8 | SampleType::U8 => BitPix::UInt8,
        SampleType::I16 => BitPix::Int16,
        SampleType::U16 => BitPix::UInt16,
        SampleType::I32 => BitPix::Int32,
        SampleType::U32 => BitPix::UInt32,
        SampleType::I64 | SampleType::U64 => BitPix::Int64,
        SampleType::F32 => BitPix::Float32,
        SampleType::F64 => BitPix::Float64,
    }
}

/// Normalize FITS pixel data to a `[0,1]`-ish range, in place.
///
/// First pass always sanitizes non-finite values to 0.0: `physical_f32` maps the
/// integer `BLANK` sentinel (and float NaN/Inf nulls) to NaN, and those must not
/// survive into the pipeline for *either* type — an integer FITS carrying a BLANK
/// keyword does produce NaN here, so the integer path can't assume clean input.
///
/// The divisor must be **frame-independent**: every frame in a stack has to normalize
/// against the same scale or dark-subtraction and combination silently misalign. So we
/// pick it in this order:
///
/// 1. **`DATAMAX`** (the writer's declared full-scale) when present and positive — the
///    correct, frame-independent answer for *either* type. This is what reunites, e.g.,
///    a 14-bit sensor written as `uint16` (`DATAMAX=16383`) whether stored as int or float.
/// 2. **Integer without DATAMAX**: BZERO/BSCALE was already applied, so divide by the
///    BITPIX type's max — fixed, frame-independent.
/// 3. **Float without DATAMAX**: the FITS standard defines no convention and the bit depth
///    is gone, so there is no frame-independent answer. As a single-file best-effort we
///    take the actual max and normalize only if it exceeds 2.0 (headroom for HDR/overexposed
///    [0,1] data while still catching integer-like ranges). **Frame-dependent** — multi-frame
///    float stacking needs `DATAMAX` set (or pre-normalized input) to stay consistent.
fn normalize_fits_pixels(mut pixels: Vec<f32>, bitpix: BitPix, data_max: Option<f64>) -> Vec<f32> {
    let max = pixels
        .par_iter_mut()
        .map(|p| {
            if p.is_finite() {
                *p
            } else {
                *p = 0.0;
                f32::NEG_INFINITY
            }
        })
        .reduce(|| f32::NEG_INFINITY, f32::max);

    let inv = match data_max {
        // DATAMAX is in the same (BSCALE/BZERO-applied) physical units as the pixels.
        Some(dm) if dm > 0.0 => 1.0 / dm as f32,
        _ => match bitpix.normalization_max() {
            Some(type_max) => 1.0 / type_max,
            None if max > 2.0 => 1.0 / max,
            None => return pixels,
        },
    };
    pixels.par_iter_mut().for_each(|p| *p *= inv);
    pixels
}

/// Read CFA pattern from BAYERPAT header, adjusting for ROWORDER if present.
///
/// BAYERPAT values: "RGGB", "BGGR", "GRBG", "GBRG", or "TRUE" (= RGGB).
/// ROWORDER: "TOP-DOWN" (default) or "BOTTOM-UP" (flips pattern vertically).
/// XBAYROFF/YBAYROFF: integer offsets into the Bayer matrix (shifts pattern).
fn read_cfa_from_headers(header: &Header) -> Option<CfaType> {
    use crate::io::raw::demosaic::bayer::CfaPattern;

    let bayerpat = header.get_text("BAYERPAT")?;
    let mut pattern = CfaPattern::from_bayerpat(bayerpat)?;

    // ROWORDER: if BOTTOM-UP, the first row in memory is the bottom of the image,
    // so the Bayer pattern needs to be flipped vertically.
    if let Some(roworder) = header.get_text("ROWORDER")
        && roworder.trim().eq_ignore_ascii_case("BOTTOM-UP")
    {
        pattern = pattern.flip_vertical();
    }

    // XBAYROFF/YBAYROFF: offset into the Bayer matrix.
    // An odd Y offset flips rows, an odd X offset flips columns.
    let xoff = header.get_integer("XBAYROFF").unwrap_or(0);
    let yoff = header.get_integer("YBAYROFF").unwrap_or(0);
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
fn read_ra_deg(header: &Header) -> Option<f64> {
    if let Some(ra) = header.get_real("RA") {
        return Some(ra);
    }
    if let Some(s) = header.get_text("OBJCTRA") {
        // OBJCTRA is in hours; scale the sexagesimal value to degrees.
        return parse_sexagesimal(s).map(|h| h * 15.0);
    }
    header.get_real("CRVAL1")
}

/// Read DEC in degrees from FITS headers.
///
/// Tries: `DEC` (degrees), `OBJCTDEC` (DMS string), `CRVAL2` (WCS).
fn read_dec_deg(header: &Header) -> Option<f64> {
    if let Some(dec) = header.get_real("DEC") {
        return Some(dec);
    }
    if let Some(s) = header.get_text("OBJCTDEC") {
        return parse_sexagesimal(s);
    }
    header.get_real("CRVAL2")
}

/// Parse a sexagesimal triple "±AA BB CC.cc" to its signed decimal value
/// `AA + BB/60 + CC/3600`. Accepts space- or colon-delimited fields.
///
/// DEC (degrees) uses the result directly; RA (hours) scales it by 15 at the call
/// site. The sign is taken from the first field, so "-00 30 00" is -0.5 — a bare
/// `-AA.abs()` would otherwise drop the sign on a zero-degree/hour field.
fn parse_sexagesimal(s: &str) -> Option<f64> {
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

/// Read an optional string-valued header key as an owned `String`.
fn read_text(header: &Header, key: &str) -> Option<String> {
    header.get_text(key).map(str::to_owned)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_normalize_float_already_01() {
        // Pixels already in [0, 1] — should be unchanged
        let pixels = vec![0.0, 0.25, 0.5, 0.75, 1.0];
        let result = normalize_fits_pixels(pixels.clone(), BitPix::Float32, None);
        assert_eq!(result, pixels);
    }

    #[test]
    fn test_normalize_float_hdr_headroom() {
        // max = 1.5 (HDR overexposure) — below threshold 2.0, unchanged
        let pixels = vec![0.0, 0.5, 1.0, 1.5];
        let result = normalize_fits_pixels(pixels.clone(), BitPix::Float32, None);
        assert_eq!(result, pixels);
    }

    #[test]
    fn test_normalize_float_at_threshold() {
        // max = 2.0 — at threshold, should NOT normalize (threshold is > 2.0, not >=)
        let pixels = vec![0.0, 1.0, 2.0];
        let result = normalize_fits_pixels(pixels.clone(), BitPix::Float32, None);
        assert_eq!(result, pixels);
    }

    #[test]
    fn test_normalize_float_0_65535_range() {
        // DeepSkyStacker output: [0, 65535] — should normalize by dividing by max
        // inv_max = 1.0 / 65535.0
        let pixels = vec![0.0, 32767.5, 65535.0];
        let result = normalize_fits_pixels(pixels, BitPix::Float32, None);
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
        let result = normalize_fits_pixels(pixels, BitPix::Float32, None);
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
        let result = normalize_fits_pixels(pixels, BitPix::Float32, None);
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
        let result = normalize_fits_pixels(pixels, BitPix::Float64, None);
        assert!((result[0] - 0.0).abs() < 1e-7);
        assert!((result[1] - 0.5).abs() < 1e-4);
        assert!((result[2] - 1.0).abs() < 1e-7);
    }

    #[test]
    fn test_normalize_uint16_unchanged() {
        // Integer types still use fixed normalization_max, not heuristic
        // UInt16: max = 65535.0
        let pixels = vec![0.0, 32767.5, 65535.0];
        let result = normalize_fits_pixels(pixels, BitPix::UInt16, None);
        // 0.0 / 65535.0 = 0.0
        assert!((result[0] - 0.0).abs() < 1e-7);
        // 32767.5 / 65535.0 ≈ 0.49999
        assert!((result[1] - 32767.5 / 65535.0).abs() < 1e-7);
        // 65535.0 / 65535.0 = 1.0
        assert!((result[2] - 1.0).abs() < 1e-7);
    }

    #[test]
    fn test_normalize_datamax_overrides_for_int_and_float() {
        // A 14-bit sensor written as uint16 (DATAMAX=16383). With DATAMAX honored, both the
        // integer and float encodings divide by 16383 → identical, full-range result. This is the
        // case where int (÷65535) and float (÷actual-max) would otherwise diverge.
        let pixels = vec![0.0, 8191.5, 16383.0];
        let dm = Some(16383.0);

        let as_int = normalize_fits_pixels(pixels.clone(), BitPix::UInt16, dm);
        let as_float = normalize_fits_pixels(pixels.clone(), BitPix::Float32, dm);

        for r in [&as_int, &as_float] {
            assert!((r[0] - 0.0).abs() < 1e-6);
            assert!((r[1] - 0.5).abs() < 1e-4);
            assert!((r[2] - 1.0).abs() < 1e-6);
        }
        assert_eq!(as_int, as_float, "DATAMAX reunites the int and float paths");
    }

    #[test]
    fn test_normalize_datamax_nonpositive_falls_back() {
        // A zero/negative DATAMAX is ignored — fall back to the per-type rule (UInt16 → ÷65535).
        let pixels = vec![0.0, 65535.0];
        let result = normalize_fits_pixels(pixels, BitPix::UInt16, Some(0.0));
        assert!((result[0] - 0.0).abs() < 1e-7);
        assert!((result[1] - 1.0).abs() < 1e-7); // ÷65535, not a panic on ÷0
    }

    #[test]
    fn test_normalize_float_all_zero() {
        // All zeros: max = 0.0, which is <= 2.0, so unchanged
        let pixels = vec![0.0, 0.0, 0.0];
        let result = normalize_fits_pixels(pixels.clone(), BitPix::Float32, None);
        assert_eq!(result, pixels);
    }

    #[test]
    fn test_normalize_float_single_pixel() {
        // Single pixel at 50000 → normalizes to 1.0
        let pixels = vec![50000.0];
        let result = normalize_fits_pixels(pixels, BitPix::Float32, None);
        assert!((result[0] - 1.0).abs() < 1e-7);
    }

    #[test]
    fn test_normalize_float_nan_replaced_with_zero() {
        // FITS uses NaN as null indicator for float images.
        // NaN pixels should become 0.0. Valid pixels unchanged (max=0.8 < 2.0).
        let pixels = vec![0.0, f32::NAN, 0.5, f32::NAN, 0.8];
        let result = normalize_fits_pixels(pixels, BitPix::Float32, None);
        assert_eq!(result, vec![0.0, 0.0, 0.5, 0.0, 0.8]);
    }

    #[test]
    fn test_normalize_float_inf_replaced_with_zero() {
        // +Inf and -Inf should become 0.0. Valid pixels unchanged (max=1.0 < 2.0).
        let pixels = vec![0.5, f32::INFINITY, 1.0, f32::NEG_INFINITY];
        let result = normalize_fits_pixels(pixels, BitPix::Float32, None);
        assert_eq!(result, vec![0.5, 0.0, 1.0, 0.0]);
    }

    #[test]
    fn test_normalize_float_nan_with_normalization() {
        // NaN replaced before normalization. max of valid pixels = 100.0 > 2.0.
        // inv_max = 1/100 = 0.01
        // [NaN→0.0, 50.0, 100.0] → [0.0*0.01, 50.0*0.01, 100.0*0.01] = [0.0, 0.5, 1.0]
        let pixels = vec![f32::NAN, 50.0, 100.0];
        let result = normalize_fits_pixels(pixels, BitPix::Float32, None);
        assert!((result[0] - 0.0).abs() < 1e-7);
        assert!((result[1] - 0.5).abs() < 1e-7);
        assert!((result[2] - 1.0).abs() < 1e-7);
    }

    #[test]
    fn test_normalize_float_all_nan() {
        // All NaN → all become 0.0. max of remaining = 0.0 < 2.0, no normalization.
        let pixels = vec![f32::NAN, f32::NAN, f32::NAN];
        let result = normalize_fits_pixels(pixels, BitPix::Float32, None);
        assert_eq!(result, vec![0.0, 0.0, 0.0]);
    }

    #[test]
    fn test_normalize_float64_nan_replaced() {
        // Float64 follows same path as Float32
        let pixels = vec![0.0, f32::NAN, 0.5];
        let result = normalize_fits_pixels(pixels, BitPix::Float64, None);
        assert_eq!(result, vec![0.0, 0.0, 0.5]);
    }

    #[test]
    fn test_normalize_uint16_nan_sanitized() {
        // fits-well's physical_f32 maps the integer BLANK sentinel to NaN, so the
        // integer path must sanitize too: NaN → 0.0 before the fixed-max divide.
        let pixels = vec![0.0, f32::NAN, 65535.0];
        let result = normalize_fits_pixels(pixels, BitPix::UInt16, None);
        assert!((result[0] - 0.0).abs() < 1e-7);
        assert_eq!(result[1], 0.0); // BLANK → 0.0, not propagated as NaN
        assert!((result[2] - 1.0).abs() < 1e-7);
    }

    #[test]
    fn test_parse_sexagesimal_hms_to_ra_deg() {
        // RA is in hours; the call site scales the triple by 15 to get degrees.
        // M42: 05h 35m 17.3s = (5 + 35/60 + 17.3/3600) * 15 = 83.82208333... deg
        let expected = (5.0 + 35.0 / 60.0 + 17.3 / 3600.0) * 15.0;
        // Space- and colon-delimited forms parse identically.
        for s in ["05 35 17.3", "05:35:17.3"] {
            let deg = parse_sexagesimal(s).unwrap() * 15.0;
            assert!(
                (deg - expected).abs() < 1e-10,
                "{s}: got {deg}, expected {expected}"
            );
        }
        // Zero triple → 0.
        assert!((parse_sexagesimal("00 00 00.0").unwrap() * 15.0).abs() < 1e-10);
    }

    #[test]
    fn test_parse_sexagesimal_dms_to_dec_deg() {
        // DEC uses the triple directly (degrees).
        // M42: -05° 23' 28.0" = -(5 + 23/60 + 28/3600) = -5.39111...
        let neg = parse_sexagesimal("-05 23 28.0").unwrap();
        assert!((neg - -(5.0 + 23.0 / 60.0 + 28.0 / 3600.0)).abs() < 1e-10);
        // Colon-delimited positive.
        let pos = parse_sexagesimal("+45:30:15.5").unwrap();
        assert!((pos - (45.0 + 30.0 / 60.0 + 15.5 / 3600.0)).abs() < 1e-10);
        // Sign must survive a zero-degree field: -00° 30' = -0.5, not +0.5.
        assert!((parse_sexagesimal("-00 30 00.0").unwrap() - -0.5).abs() < 1e-10);
    }

    #[test]
    fn test_parse_sexagesimal_invalid() {
        assert!(parse_sexagesimal("05 35").is_none()); // only 2 fields
        assert!(parse_sexagesimal("").is_none());
        assert!(parse_sexagesimal("abc def ghi").is_none());
    }
}
