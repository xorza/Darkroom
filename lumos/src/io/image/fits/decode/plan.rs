use std::mem::size_of;
use std::path::Path;

use fits_well::header::Header;
use fits_well::image::{Bitpix as FitsBitpix, SampleType};
use fits_well::io::{BLOCK_SIZE, Hdu, HduKind};

use crate::io::image::error::ImageError;
use crate::io::image::fits::error::{fits_err, fits_unsupported};
use crate::io::image::fits::options::{FitsCubeInterpretation, FitsFloatScale};
use crate::io::image::image_dimensions::ImageDimensions;
use crate::io::image::image_metadata::BitPix;

const FITS_DECODE_CHUNK_BYTES: usize = 4 * 1024 * 1024;

/// Above this, a floating-point HDU's `DATAMAX` is read as declaring an ADU saturation level rather
/// than a normalized one. Siril's threshold, and well clear of both the `[0, 1]` convention and the
/// slight overshoot an interpolated or stacked frame can carry past unity.
const FLOAT_ADU_DATAMAX_MIN: f64 = 10.0;

/// What such a frame is divided by: the 16-bit full scale, which is the depth essentially every
/// camera that writes ADU into a float FITS digitizes at. Siril and PixInsight both use it.
const FLOAT_ADU_DIVISOR: f32 = 65_535.0;

#[derive(Debug, Clone, Copy)]
pub(super) struct FitsHduDescription<'a> {
    header: &'a Header,
    kind: HduKind,
    source_bytes: u64,
}

impl<'a> FitsHduDescription<'a> {
    pub(super) fn from_hdu(path: &Path, hdu: &'a Hdu) -> Result<Self, ImageError> {
        let source_bytes = padded_data_bytes(path, hdu.data_bytes)?;
        Ok(Self {
            header: &hdu.header,
            kind: hdu.kind,
            source_bytes,
        })
    }
}

#[derive(Debug)]
pub(super) struct FitsDecodePlan {
    pub(super) shape: Vec<usize>,
    pub(super) dimensions: ImageDimensions,
    pub(super) bitpix: BitPix,
    pub(super) scaling: fits_well::image::Scaling,
    /// Divide a physical sample by this to reach the pipeline's `[0, 1]` domain; multiply a
    /// decoded sample by it to recover the physical value. See [`sample_divisor`].
    pub(super) sample_divisor: f32,
    pub(super) source_bytes: u64,
    pub(super) decoded_bytes: u64,
    pub(super) peak_bytes: u64,
    pub(super) rows_per_chunk: usize,
}

/// What one full-scale span of the stored integer type measures, in physical units.
///
/// The pipeline's linear domain is `[0, 1]`, so an integer FITS is divided by the span its own
/// `BITPIX` and `BSCALE` declare: `|BSCALE| × (2^bits − 1)`. That maps the FITS unsigned
/// convention (`BITPIX = 16`, `BZERO = 2¹⁵`) exactly onto `[0, 1]`, and puts a signed frame on
/// `[-0.5, 0.5]` around its own zero.
///
/// Only the scale is applied — never an offset, and never a clamp. Offsetting would move a signed
/// frame's zero point, and clamping would cut the sub-pedestal noise tail that the calibration
/// path depends on (see [`crate::io::raw`]'s unclamped normalization).
///
/// A floating-point `BITPIX` declares no full scale, so the only evidence available is `DATAMAX`: a
/// declared saturation level above [`FLOAT_ADU_DATAMAX_MIN`] means the samples are ADU rather than
/// `[0, 1]` and they are divided by [`FLOAT_ADU_DIVISOR`], the threshold and divisor Siril uses.
/// Anything else — a `DATAMAX` of about 1, or none at all — is taken as already normalized, which is
/// PixInsight's default for a float FITS and what keeps a Lumos-written master round-tripping.
///
/// The test is on the *header*, never on the pixels, and that is where this departs from Siril:
/// with `DATAMAX` absent it scans the data instead (three sampled pixels on the partial-read path,
/// which is why its full and partial reads can disagree about the same file). A divisor read off
/// each frame's own extrema differs frame to frame, which is exactly what
/// [`crate::stacking::combine`] now rejects a frame set for. An unnormalized float FITS carrying no
/// `DATAMAX` therefore reaches the pipeline as it stands; the display stage measures its own range
/// rather than the decoder guessing one.
fn sample_divisor(
    path: &Path,
    header: &Header,
    stored: FitsBitpix,
    scaling: &fits_well::image::Scaling,
    float_scale: FitsFloatScale,
) -> Result<f32, ImageError> {
    let steps = match stored {
        FitsBitpix::U8 => f64::from(u8::MAX),
        FitsBitpix::I16 => f64::from(u16::MAX),
        FitsBitpix::I32 => f64::from(u32::MAX),
        FitsBitpix::I64 => u64::MAX as f64,
        FitsBitpix::F32 | FitsBitpix::F64 => {
            return match float_scale {
                FitsFloatScale::Normalized => Ok(1.0),
                FitsFloatScale::FullScale(scale) => {
                    // The caller's own figure, so it is checked here rather than trusted: a
                    // non-positive one would invert or erase the samples.
                    if !scale.is_finite() || scale <= 0.0 {
                        return Err(fits_unsupported(
                            path,
                            format!("declared floating-point full scale {scale} must be positive"),
                        ));
                    }
                    Ok(scale)
                }
                FitsFloatScale::Auto => {
                    let data_max = header
                        .get_real("DATAMAX")
                        .map_err(|source| fits_err(path, source))?;
                    Ok(match data_max {
                        Some(max) if max > FLOAT_ADU_DATAMAX_MIN => FLOAT_ADU_DIVISOR,
                        _ => 1.0,
                    })
                }
            };
        }
    };
    // File-derived metadata: a corrupt or hand-edited header can carry any BSCALE, and a zero or
    // non-finite one leaves no span to normalize into. Reject rather than emit infinities.
    let bscale = scaling.bscale;
    if !bscale.is_finite() || bscale == 0.0 {
        return Err(fits_unsupported(
            path,
            format!("BSCALE {bscale} leaves no scale to normalize integer samples by"),
        ));
    }
    let divisor = bscale.abs() * steps;
    if !divisor.is_finite() || divisor <= 0.0 {
        return Err(fits_unsupported(
            path,
            format!("BSCALE {bscale} overflows the normalization scale for {stored:?} samples"),
        ));
    }
    Ok(divisor as f32)
}

pub(super) fn preflight_fits_image(
    path: &Path,
    hdu: FitsHduDescription<'_>,
    cube: FitsCubeInterpretation,
    float_scale: FitsFloatScale,
    memory_limit_bytes: u64,
) -> Result<FitsDecodePlan, ImageError> {
    if !matches!(
        hdu.kind,
        HduKind::Primary | HduKind::Image | HduKind::CompressedImage
    ) {
        return Err(fits_unsupported(path, "selected HDU is not an image"));
    }

    let shape = if hdu.kind == HduKind::CompressedImage {
        compressed_shape(hdu.header).map_err(|source| fits_err(path, source))?
    } else {
        hdu.header.axes().map_err(|source| fits_err(path, source))?
    };
    let dimensions = dimensions_from_shape(path, &shape, cube)?;
    let stored_bitpix = if hdu.kind == HduKind::CompressedImage {
        let code = hdu
            .header
            .get_integer("ZBITPIX")
            .map_err(|source| fits_err(path, source))?
            .ok_or_else(|| fits_unsupported(path, "compressed image is missing ZBITPIX"))?;
        FitsBitpix::from_code(code).map_err(|source| fits_err(path, source))?
    } else {
        hdu.header
            .bitpix()
            .map_err(|source| fits_err(path, source))?
    };
    let scaling = hdu
        .header
        .scaling()
        .map_err(|source| fits_err(path, source))?;
    let bitpix = map_bitpix(SampleType::from_scaling(stored_bitpix, &scaling));
    let sample_divisor = sample_divisor(path, hdu.header, stored_bitpix, &scaling, float_scale)?;
    let decoded_bytes = checked_size_bytes(
        path,
        dimensions.sample_count(),
        size_of::<f32>(),
        "decoded FITS output",
    )?;
    let row_samples = dimensions.width();
    let row_f32_bytes = row_samples
        .checked_mul(size_of::<f32>())
        .ok_or_else(|| fits_unsupported(path, "FITS output row size overflows usize"))?;
    let rows_per_chunk = (FITS_DECODE_CHUNK_BYTES / row_f32_bytes.max(1))
        .max(1)
        .min(dimensions.height());
    let chunk_samples = row_samples
        .checked_mul(rows_per_chunk)
        .ok_or_else(|| fits_unsupported(path, "FITS decode chunk size overflows usize"))?;
    let native_chunk_bytes = checked_size_bytes(
        path,
        chunk_samples,
        stored_bitpix.elem_size(),
        "FITS native decode chunk",
    )?;
    let physical_chunk_bytes = checked_size_bytes(
        path,
        chunk_samples,
        size_of::<f32>(),
        "FITS physical decode chunk",
    )?;
    let peak_bytes = decoded_bytes
        .checked_add(native_chunk_bytes)
        .and_then(|bytes| bytes.checked_add(native_chunk_bytes))
        .and_then(|bytes| bytes.checked_add(physical_chunk_bytes))
        .and_then(|bytes| {
            if hdu.kind == HduKind::CompressedImage {
                bytes.checked_add(hdu.source_bytes.checked_mul(2)?)
            } else {
                Some(bytes)
            }
        })
        .ok_or_else(|| fits_unsupported(path, "FITS peak memory size overflows u64"))?;

    enforce_fits_budget(
        path,
        "stored data unit",
        hdu.source_bytes,
        memory_limit_bytes,
    )?;
    enforce_fits_budget(path, "decoded output", decoded_bytes, memory_limit_bytes)?;
    enforce_fits_budget(
        path,
        "estimated peak memory",
        peak_bytes,
        memory_limit_bytes,
    )?;

    Ok(FitsDecodePlan {
        shape,
        dimensions,
        bitpix,
        scaling,
        sample_divisor,
        source_bytes: hdu.source_bytes,
        decoded_bytes,
        peak_bytes,
        rows_per_chunk,
    })
}

fn checked_size_bytes(
    path: &Path,
    elements: usize,
    element_bytes: usize,
    name: &str,
) -> Result<u64, ImageError> {
    elements
        .checked_mul(element_bytes)
        .and_then(|bytes| u64::try_from(bytes).ok())
        .ok_or_else(|| fits_unsupported(path, format!("{name} size overflows usize")))
}

fn enforce_fits_budget(
    path: &Path,
    name: &str,
    required: u64,
    memory_limit_bytes: u64,
) -> Result<(), ImageError> {
    if required > memory_limit_bytes {
        return Err(fits_unsupported(
            path,
            format!(
                "{name} requires {required} bytes, exceeding the FITS load budget of {} bytes",
                memory_limit_bytes
            ),
        ));
    }
    Ok(())
}

fn padded_data_bytes(path: &Path, bytes: u64) -> Result<u64, ImageError> {
    if bytes == 0 {
        return Ok(0);
    }
    bytes
        .checked_add(BLOCK_SIZE as u64 - 1)
        .map(|padded| padded / BLOCK_SIZE as u64 * BLOCK_SIZE as u64)
        .ok_or_else(|| fits_unsupported(path, "FITS padded data-unit size overflows u64"))
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

pub(super) fn dimensions_from_shape(
    path: &Path,
    shape: &[usize],
    cube: FitsCubeInterpretation,
) -> Result<ImageDimensions, ImageError> {
    if shape.contains(&0) {
        return Err(fits_unsupported(
            path,
            format!("FITS image axes must be nonzero, got {shape:?}"),
        ));
    }
    let (width, height, channels) = match shape {
        [width, height] => (*width, *height, 1),
        [width, height, 1] => (*width, *height, 1),
        [width, height, 3] if cube == FitsCubeInterpretation::Rgb => (*width, *height, 3),
        [_, _, 3] => Err(fits_unsupported(
            path,
            "three-plane FITS cube requires FitsCubeInterpretation::Rgb",
        ))?,
        [_, _, channels] => Err(fits_unsupported(
            path,
            format!("Unsupported channel count (NAXIS3): {channels}"),
        ))?,
        _ => {
            return Err(fits_unsupported(
                path,
                format!("Unsupported number of dimensions: {}", shape.len()),
            ));
        }
    };
    let pixel_count = width
        .checked_mul(height)
        .ok_or_else(|| fits_unsupported(path, format!("FITS pixel count overflows: {shape:?}")))?;
    pixel_count
        .checked_mul(channels)
        .ok_or_else(|| fits_unsupported(path, format!("FITS sample count overflows: {shape:?}")))?;
    Ok(ImageDimensions::new((width, height), channels))
}

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

#[cfg(test)]
pub(super) mod internals {
    use fits_well::header::Header;
    use fits_well::io::HduKind;

    use crate::io::image::fits::decode::plan::FitsHduDescription;

    pub(crate) fn description(
        header: &Header,
        kind: HduKind,
        source_bytes: u64,
    ) -> FitsHduDescription<'_> {
        FitsHduDescription {
            header,
            kind,
            source_bytes,
        }
    }
}
