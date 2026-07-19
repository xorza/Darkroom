use std::path::Path;

use fits_well::FitsReader;
use fits_well::header::Header;
use fits_well::image::SampleType;
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
        (raw.metadata().shape.to_vec(), bitpix, pixels)
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
    let metadata = read_metadata(header, shape, bitpix).map_err(|source| fits_err(path, source))?;
    let pixels = prepare_fits_pixels(pixels, bitpix, metadata.data_max).map_err(|summary| {
        fits_unsupported(
            path,
            format!(
                "image contains {} null/non-finite pixels; first at linear index {}",
                summary.count, summary.first_index
            ),
        )
    })?;

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

fn read_metadata(
    header: &Header,
    header_dimensions: Vec<usize>,
    bitpix: BitPix,
) -> fits_well::Result<AstroImageMetadata> {
    Ok(AstroImageMetadata {
        object: read_text(header, "OBJECT")?,
        instrument: read_text(header, "INSTRUME")?,
        telescope: read_text(header, "TELESCOP")?,
        date_obs: read_text(header, "DATE-OBS")?,
        exposure_time: header.get_real("EXPTIME")?,
        iso: read_u32(header, "ISOSPEED")?,
        bitpix,
        header_dimensions,
        cfa_type: read_cfa_from_headers(header)?,
        camera_white_balance: None,
        filter: read_text(header, "FILTER")?,
        gain: header.get_real("GAIN")?,
        egain: header.get_real("EGAIN")?,
        ccd_temp: first_real(header, "CCD-TEMP", "CCDTEMP")?,
        image_type: first_text(header, "IMAGETYP", "FRAME")?,
        xbinning: read_i32(header, "XBINNING")?,
        ybinning: read_i32(header, "YBINNING")?,
        set_temp: header.get_real("SET-TEMP")?,
        offset: read_i32(header, "OFFSET")?,
        focal_length: header.get_real("FOCALLEN")?,
        airmass: header.get_real("AIRMASS")?,
        ra_deg: read_ra_deg(header)?,
        dec_deg: read_dec_deg(header)?,
        pixel_size_x: header.get_real("XPIXSZ")?,
        pixel_size_y: header.get_real("YPIXSZ")?,
        data_max: header.get_real("DATAMAX")?,
        calibrated: false,
    })
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct NullSummary {
    count: usize,
    first_index: usize,
}

/// Validate FITS pixels and normalize integer sample types.
///
/// Floating-point FITS values are already physical values and remain unchanged. `DATAMAX` is
/// descriptive image metadata, not a guaranteed sensor full-scale, so using it as a divisor can
/// give sibling frames different scales.
///
/// Integer samples use a positive finite `DATAMAX` when supplied, otherwise their fixed BITPIX
/// range. BZERO/BSCALE has already been applied before this step.
///
/// `physical_f32` represents integer `BLANK` and floating nulls as non-finite values. Until
/// `AstroImage` carries a validity plane, accepting those values would either poison arithmetic or
/// invent valid zero-valued samples, so the entire image is rejected with a summary.
fn prepare_fits_pixels(
    mut pixels: Vec<f32>,
    bitpix: BitPix,
    data_max: Option<f64>,
) -> Result<Vec<f32>, NullSummary> {
    let nulls = pixels
        .par_iter()
        .enumerate()
        .filter_map(|(index, pixel)| {
            (!pixel.is_finite()).then_some(NullSummary {
                count: 1,
                first_index: index,
            })
        })
        .reduce_with(|left, right| NullSummary {
            count: left.count + right.count,
            first_index: left.first_index.min(right.first_index),
        });
    if let Some(summary) = nulls {
        return Err(summary);
    }

    let inv = match bitpix {
        BitPix::Float32 | BitPix::Float64 => None,
        _ => match data_max {
            Some(dm) if dm.is_finite() && dm > 0.0 => Some(1.0 / dm as f32),
            _ => bitpix.normalization_max().map(|type_max| 1.0 / type_max),
        },
    };
    if let Some(inv) = inv {
        pixels.par_iter_mut().for_each(|pixel| *pixel *= inv);
    }
    Ok(pixels)
}

/// Read CFA pattern from BAYERPAT header, adjusting for ROWORDER if present.
///
/// BAYERPAT values: "RGGB", "BGGR", "GRBG", "GBRG", or "TRUE" (= RGGB).
/// ROWORDER: "TOP-DOWN" (default) or "BOTTOM-UP" (flips pattern vertically).
/// XBAYROFF/YBAYROFF: integer offsets into the Bayer matrix (shifts pattern).
fn read_cfa_from_headers(header: &Header) -> fits_well::Result<Option<CfaType>> {
    use crate::io::raw::demosaic::bayer::CfaPattern;

    let Some(bayerpat) = header.get_text("BAYERPAT")? else {
        return Ok(None);
    };
    let Some(mut pattern) = CfaPattern::from_bayerpat(bayerpat) else {
        return Ok(None);
    };

    // ROWORDER: if BOTTOM-UP, the first row in memory is the bottom of the image,
    // so the Bayer pattern needs to be flipped vertically.
    if let Some(roworder) = header.get_text("ROWORDER")?
        && roworder.trim().eq_ignore_ascii_case("BOTTOM-UP")
    {
        pattern = pattern.flip_vertical();
    }

    // XBAYROFF/YBAYROFF: offset into the Bayer matrix.
    // An odd Y offset flips rows, an odd X offset flips columns.
    let xoff = header.get_integer("XBAYROFF")?.unwrap_or(0);
    let yoff = header.get_integer("YBAYROFF")?.unwrap_or(0);
    if yoff & 1 != 0 {
        pattern = pattern.flip_vertical();
    }
    if xoff & 1 != 0 {
        pattern = pattern.flip_horizontal();
    }

    Ok(Some(CfaType::Bayer(pattern)))
}

/// Read RA in degrees from FITS headers.
///
/// Tries: `RA` (degrees, NINA/SGP), `OBJCTRA` (HMS string, MaximDL/ASCOM),
/// `CRVAL1` (WCS reference point, plate-solved images).
fn read_ra_deg(header: &Header) -> fits_well::Result<Option<f64>> {
    if let Some(ra) = header.get_real("RA")? {
        return Ok(Some(ra));
    }
    if let Some(s) = header.get_text("OBJCTRA")? {
        // OBJCTRA is in hours; scale the sexagesimal value to degrees.
        return Ok(parse_sexagesimal(s).map(|h| h * 15.0));
    }
    header.get_real("CRVAL1")
}

/// Read DEC in degrees from FITS headers.
///
/// Tries: `DEC` (degrees), `OBJCTDEC` (DMS string), `CRVAL2` (WCS).
fn read_dec_deg(header: &Header) -> fits_well::Result<Option<f64>> {
    if let Some(dec) = header.get_real("DEC")? {
        return Ok(Some(dec));
    }
    if let Some(s) = header.get_text("OBJCTDEC")? {
        return Ok(parse_sexagesimal(s));
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
fn read_text(header: &Header, key: &str) -> fits_well::Result<Option<String>> {
    Ok(header.get_text(key)?.map(str::to_owned))
}

fn first_text(header: &Header, first: &str, second: &str) -> fits_well::Result<Option<String>> {
    match read_text(header, first)? {
        Some(value) => Ok(Some(value)),
        None => read_text(header, second),
    }
}

fn first_real(header: &Header, first: &str, second: &str) -> fits_well::Result<Option<f64>> {
    match header.get_real(first)? {
        Some(value) => Ok(Some(value)),
        None => header.get_real(second),
    }
}

fn read_u32(header: &Header, key: &'static str) -> fits_well::Result<Option<u32>> {
    header
        .get_integer(key)?
        .map(|value| {
            u32::try_from(value).map_err(|_| fits_well::FitsError::KeywordOutOfRange { name: key })
        })
        .transpose()
}

fn read_i32(header: &Header, key: &'static str) -> fits_well::Result<Option<i32>> {
    header
        .get_integer(key)?
        .map(|value| {
            i32::try_from(value).map_err(|_| fits_well::FitsError::KeywordOutOfRange { name: key })
        })
        .transpose()
}

#[cfg(test)]
mod tests {
    use crate::io::astro_image::fits::*;

    #[test]
    fn float_without_datamax_preserves_physical_values() {
        let pixels = vec![-5.0, 0.0, 0.5, 2.0, 255.0, 65_535.0];

        for bitpix in [BitPix::Float32, BitPix::Float64] {
            assert_eq!(
                prepare_fits_pixels(pixels.clone(), bitpix, None).unwrap(),
                pixels,
                "{bitpix:?}"
            );
        }
    }

    #[test]
    fn integer_without_datamax_uses_bitpix_range() {
        let result =
            prepare_fits_pixels(vec![0.0, 32_767.5, 65_535.0], BitPix::UInt16, None).unwrap();

        assert_eq!(result, vec![0.0, 0.5, 1.0]);
    }

    #[test]
    fn valid_datamax_sets_integer_scale() {
        assert_eq!(
            prepare_fits_pixels(vec![0.0, 50.0, 100.0], BitPix::UInt16, Some(100.0)).unwrap(),
            vec![0.0, 0.5, 1.0]
        );
    }

    #[test]
    fn float_datamax_remains_metadata_only() {
        let pixels = vec![-5.0, 0.0, 50.0, 100.0];

        for bitpix in [BitPix::Float32, BitPix::Float64] {
            assert_eq!(
                prepare_fits_pixels(pixels.clone(), bitpix, Some(100.0)).unwrap(),
                pixels,
                "{bitpix:?}"
            );
        }
    }

    #[test]
    fn invalid_datamax_uses_sample_type_fallback() {
        for datamax in [0.0, f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            assert_eq!(
                prepare_fits_pixels(vec![0.0, 32_767.5, 65_535.0], BitPix::UInt16, Some(datamax),)
                    .unwrap(),
                vec![0.0, 0.5, 1.0]
            );
        }
    }

    #[test]
    fn non_finite_pixels_return_exact_summary_for_every_sample_type() {
        let pixels = vec![0.0, f32::NAN, 5.0, f32::INFINITY, f32::NEG_INFINITY];

        for bitpix in [BitPix::UInt16, BitPix::Float32, BitPix::Float64] {
            assert_eq!(
                prepare_fits_pixels(pixels.clone(), bitpix, None).unwrap_err(),
                NullSummary {
                    count: 3,
                    first_index: 1,
                },
                "{bitpix:?}"
            );
        }
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
