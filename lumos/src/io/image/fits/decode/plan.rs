use std::mem::size_of;
use std::path::Path;

use fits_well::header::Header;
use fits_well::image::{Bitpix as FitsBitpix, SampleType};
use fits_well::io::{BLOCK_SIZE, Hdu, HduKind};

use crate::io::image::error::ImageError;
use crate::io::image::fits::error::{fits_err, fits_unsupported};
use crate::io::image::fits::options::FitsCubeInterpretation;
use crate::io::image::{BitPix, ImageDimensions};

const FITS_DECODE_CHUNK_BYTES: usize = 4 * 1024 * 1024;

#[derive(Debug, Clone, Copy)]
pub(crate) struct FitsHduDescription<'a> {
    header: &'a Header,
    kind: HduKind,
    source_bytes: u64,
}

impl<'a> FitsHduDescription<'a> {
    pub(crate) fn from_hdu(path: &Path, hdu: &'a Hdu) -> Result<Self, ImageError> {
        let source_bytes = padded_data_bytes(path, hdu.data_bytes)?;
        Ok(Self {
            header: &hdu.header,
            kind: hdu.kind,
            source_bytes,
        })
    }
}

#[derive(Debug)]
pub(crate) struct FitsDecodePlan {
    pub(crate) shape: Vec<usize>,
    pub(crate) dimensions: ImageDimensions,
    pub(crate) bitpix: BitPix,
    pub(crate) scaling: fits_well::image::Scaling,
    pub(crate) source_bytes: u64,
    pub(crate) decoded_bytes: u64,
    pub(crate) peak_bytes: u64,
    pub(crate) rows_per_chunk: usize,
}

pub(crate) fn preflight_fits_image(
    path: &Path,
    hdu: FitsHduDescription<'_>,
    cube: FitsCubeInterpretation,
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

pub(crate) fn dimensions_from_shape(
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
pub(crate) mod test_support {
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
