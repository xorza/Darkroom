use std::fs::File;
use std::path::Path;

use fits_well::FitsReader;
use fits_well::header::Header;
use fits_well::image::SampleType;
use fits_well::io::HduKind;
use rayon::prelude::*;

use crate::io::image::cfa::CfaType;
use crate::io::image::error::ImageError;
use crate::io::image::linear::LinearImage;
use crate::io::image::{
    BitPix, ColorProvenance, DecoderProvenance, DemosaicProvenance, ImageDimensions, ImageMetadata,
    ImageProvenance, SourceContainer, TransferProvenance,
};

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
pub(crate) fn load_fits(path: &Path) -> Result<LinearImage, ImageError> {
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
    let (shape, bitpix, scaling, pixels) = {
        let raw = reader.read_image(index).map_err(|e| fits_err(path, e))?;
        let bitpix = map_bitpix(raw.sample_type());
        let scaling = raw.metadata().scaling;
        // `physical_f32` applies BSCALE/BZERO and maps the integer BLANK to NaN (the
        // effective values cfitsio's f32 read produced), narrowing in a single pass.
        let pixels: Vec<f32> = raw.physical_f32();
        (raw.metadata().shape.to_vec(), bitpix, scaling, pixels)
    };

    // fits-well reports shape NAXIS1-first: [width, height, (channels)]. All shapes other than
    // 2D or 3D-with-NAXIS3∈{1,3} are rejected as `Err` — never asserted — because `shape` comes
    // from untrusted file input (the `n` arm also covers the 0/1-dimension cases).
    let img_dims = dimensions_from_shape(path, &shape)?;

    // Shape↔buffer mismatch means a corrupt/unsupported file, not a logic error — return `Err`.
    let plane_size = img_dims.size.x * img_dims.size.y;
    if pixels.len() != plane_size * img_dims.channels {
        return Err(fits_unsupported(
            path,
            format!("pixel count {} doesn't match shape {shape:?}", pixels.len()),
        ));
    }

    let header = &reader.hdus()[index].header;
    let mut metadata =
        read_metadata(header, shape, bitpix).map_err(|source| fits_err(path, source))?;
    metadata.provenance = Some(ImageProvenance {
        container: SourceContainer::Fits,
        decoder: DecoderProvenance::FitsWell,
        transfer: TransferProvenance::FitsPhysical {
            bscale: scaling.bscale,
            bzero: scaling.bzero,
            unit: read_text(header, "BUNIT").map_err(|source| fits_err(path, source))?,
        },
        color: if metadata.cfa_type.is_some() {
            ColorProvenance::SensorCfa
        } else if img_dims.is_grayscale() {
            ColorProvenance::Monochrome
        } else {
            ColorProvenance::Unspecified
        },
        clipped: false,
        demosaic: DemosaicProvenance::None,
    });
    let pixels = validate_fits_pixels(pixels).map_err(|summary| {
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
    let mut image = if img_dims.channels == 3 {
        let channels = pixels.chunks_exact(plane_size).map(|c| c.to_vec());
        LinearImage::from_planar_channels(img_dims, channels)
    } else {
        LinearImage::from_pixels(img_dims, pixels)
    };
    image.metadata = metadata;
    Ok(image)
}

pub(crate) fn fits_dimensions(path: &Path) -> Result<ImageDimensions, ImageError> {
    let file = File::open(path).map_err(|source| ImageError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    let reader = FitsReader::open(file).map_err(|source| fits_err(path, source))?;
    let index = *reader
        .image_indices()
        .first()
        .ok_or_else(|| fits_unsupported(path, "no image HDU found"))?;
    let hdu = &reader.hdus()[index];
    let shape = match hdu.kind {
        HduKind::CompressedImage => {
            compressed_shape(&hdu.header).map_err(|source| fits_err(path, source))?
        }
        _ => hdu.header.axes().map_err(|source| fits_err(path, source))?,
    };
    dimensions_from_shape(path, &shape)
}

fn compressed_shape(header: &Header) -> fits_well::Result<Vec<usize>> {
    let rank = header
        .get_integer("ZNAXIS")?
        .ok_or(fits_well::FitsError::MissingKeyword { name: "ZNAXIS" })?;
    let rank = usize::try_from(rank)
        .ok()
        .filter(|rank| *rank <= 999)
        .ok_or(fits_well::FitsError::KeywordOutOfRange { name: "ZNAXIS" })?;
    (1..=rank)
        .map(|axis| {
            let key = format!("ZNAXIS{axis}");
            let value = header
                .get_integer(&key)?
                .ok_or(fits_well::FitsError::MissingKeyword { name: "ZNAXISn" })?;
            usize::try_from(value)
                .map_err(|_| fits_well::FitsError::KeywordOutOfRange { name: "ZNAXISn" })
        })
        .collect()
}

fn dimensions_from_shape(path: &Path, shape: &[usize]) -> Result<ImageDimensions, ImageError> {
    match shape {
        [width, height] => Ok(ImageDimensions::new((*width, *height), 1)),
        [width, height, channels] if *channels == 1 || *channels == 3 => {
            Ok(ImageDimensions::new((*width, *height), *channels))
        }
        [_, _, channels] => Err(fits_unsupported(
            path,
            format!("Unsupported channel count (NAXIS3): {channels}"),
        )),
        _ => Err(fits_unsupported(
            path,
            format!("Unsupported number of dimensions: {}", shape.len()),
        )),
    }
}

fn read_metadata(
    header: &Header,
    header_dimensions: Vec<usize>,
    bitpix: BitPix,
) -> fits_well::Result<ImageMetadata> {
    Ok(ImageMetadata {
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
        provenance: None,
        calibrated: false,
    })
}

/// Map fits-well's effective `SampleType` to lumos's `BitPix`.
///
/// `SampleType` already resolves the FITS unsigned/signed-byte BZERO conventions. Lumos has no
/// signed-byte or unsigned-64 metadata variant, so `I8` folds into `UInt8` and `U64` into `Int64`.
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

/// `physical_f32` represents integer `BLANK` and floating nulls as non-finite values. Until
/// `LinearImage` carries a validity plane, accepting those values would either poison arithmetic or
/// invent valid zero-valued samples, so the entire image is rejected with a summary.
fn validate_fits_pixels(pixels: Vec<f32>) -> Result<Vec<f32>, NullSummary> {
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
    use crate::io::image::fits::*;

    #[test]
    fn finite_validation_preserves_every_physical_value() {
        let pixels = vec![-5.0, 0.0, 0.5, 2.0, 255.0, 65_535.0];
        assert_eq!(validate_fits_pixels(pixels.clone()).unwrap(), pixels);
    }

    #[test]
    fn non_finite_pixels_return_exact_summary_for_every_sample_type() {
        let pixels = vec![0.0, f32::NAN, 5.0, f32::INFINITY, f32::NEG_INFINITY];

        assert_eq!(
            validate_fits_pixels(pixels).unwrap_err(),
            NullSummary {
                count: 3,
                first_index: 1,
            }
        );
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
