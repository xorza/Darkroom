use std::fs::File;
use std::path::Path;

use fits_well::FitsReader;
use fits_well::io::SliceReader;

use crate::io::image::cfa::{CfaFrameInfo, CfaImage, QUANTIZATION_SIGMA_PER_STEP};
use crate::io::image::error::ImageError;
use crate::io::image::fits::cfa::{validate_cfa_container_format, validate_cfa_image_header};
use crate::io::image::fits::decode::plan::FitsHduDescription;
use crate::io::image::fits::error::{fits_err, fits_unsupported};
use crate::io::image::fits::metadata::{read_cfa_from_headers, read_quantization_sigma};
use crate::io::image::fits::options::{FitsChecksumPolicy, FitsCubeInterpretation};
use crate::io::image::fits::provenance::{
    FitsChecksumProvenance, FitsChecksumState, FitsTransferProvenance,
};
use crate::io::image::linear::LinearImage;
use crate::io::image::linear_pixels::LinearPixels;
use crate::io::image::{
    BitPix, ColorProvenance, ImageMetadata, ImageProvenance, LoadContext, TransferProvenance,
    scientific_rejection,
};
use crate::io::raw::demosaic::DemosaicError;

pub(crate) mod pixels;
pub(crate) mod plan;
pub(crate) mod selection;

#[derive(Debug)]
pub(crate) struct DecodedFitsImage {
    metadata: ImageMetadata,
    pixels: LinearPixels,
}

impl DecodedFitsImage {
    fn into_linear(self, path: &Path) -> Result<LinearImage, ImageError> {
        if self.metadata.cfa_type.is_some() {
            return Err(scientific_rejection(
                path,
                "mosaic FITS must be loaded as CfaImage and calibrated before demosaicing",
            ));
        }

        Ok(LinearImage {
            metadata: self.metadata,
            pixels: self.pixels,
        })
    }

    fn into_cfa(
        self,
        path: &Path,
        declared_quantization_sigma: Option<f32>,
    ) -> Result<CfaImage, ImageError> {
        if !self.pixels.dimensions().is_grayscale() {
            return Err(fits_unsupported(
                path,
                "scientific CFA input must have exactly one image plane",
            ));
        }
        if self.metadata.cfa_type.is_none() {
            return Err(fits_unsupported(
                path,
                "scientific CFA FITS input is missing validated CFA pattern metadata",
            ));
        }

        let quantization_sigma = declared_quantization_sigma.or_else(|| {
            match (&self.metadata.bitpix, &self.metadata.provenance) {
                (
                    BitPix::UInt8
                    | BitPix::Int16
                    | BitPix::UInt16
                    | BitPix::Int32
                    | BitPix::UInt32
                    | BitPix::Int64,
                    Some(ImageProvenance {
                        transfer:
                            TransferProvenance::FitsPhysical(FitsTransferProvenance { bscale, .. }),
                        ..
                    }),
                ) => Some(bscale.abs() as f32 * QUANTIZATION_SIGMA_PER_STEP),
                _ => None,
            }
        });
        let Self {
            mut metadata,
            pixels,
            ..
        } = self;
        if let Some(provenance) = &mut metadata.provenance {
            provenance.color = ColorProvenance::SensorCfa;
        }
        Ok(CfaImage {
            data: pixels.into_l(),
            metadata,
            quantization_sigma,
        })
    }
}

/// Load an already-linear astronomical image from a FITS file.
pub(crate) fn load_linear_fits(
    path: &Path,
    context: &LoadContext,
) -> Result<LinearImage, ImageError> {
    read_selected_image(path, context)?.into_linear(path)
}

pub(crate) fn load_preview_fits(
    path: &Path,
    context: &LoadContext,
) -> Result<LinearImage, ImageError> {
    let decoded = read_selected_image(path, context)?;
    if decoded.metadata.cfa_type.is_some() {
        Ok(decoded
            .into_cfa(path, None)?
            .demosaic(&context.cancel)
            .map_err(|source| match source {
                DemosaicError::Cancelled => ImageError::Cancelled {
                    path: path.to_path_buf(),
                },
                DemosaicError::InvalidXTransPattern(source) => {
                    fits_unsupported(path, source.to_string())
                }
            })?)
    } else {
        decoded.into_linear(path)
    }
}

fn read_selected_image(path: &Path, context: &LoadContext) -> Result<DecodedFitsImage, ImageError> {
    context.check_cancelled(path)?;
    let file = File::open(path).map_err(|source| ImageError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    let mut reader = FitsReader::open(file).map_err(|source| fits_err(path, source))?;
    context.check_cancelled(path)?;

    let selected = selection::select_image_hdu(path, reader.hdus(), &context.fits.hdu)?;
    let hdu = &reader.hdus()[selected.index];
    let plan = plan::preflight_fits_image(
        path,
        FitsHduDescription::from_hdu(path, hdu)?,
        context.fits.cube,
        context.memory_limit_bytes,
    )?;
    let checksum = selection::verify_selected_checksum(
        &mut reader,
        selected.index,
        path,
        context.fits.checksum,
        context,
    )?;

    pixels::read_stream_hdu(&mut reader, selected, checksum, path, plan, context)
}

pub(crate) fn load_cfa_fits(path: &Path, context: &LoadContext) -> Result<CfaImage, ImageError> {
    context.check_cancelled(path)?;
    let file = File::open(path).map_err(|source| ImageError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    let mut reader = FitsReader::open(file).map_err(|source| fits_err(path, source))?;
    context.check_cancelled(path)?;
    validate_cfa_container_format(path, reader.hdus().first().map(|hdu| &hdu.header))?;
    let selected = selection::select_image_hdu(path, reader.hdus(), &context.fits.hdu)?;
    let hdu = &reader.hdus()[selected.index];
    let plan = plan::preflight_fits_image(
        path,
        FitsHduDescription::from_hdu(path, hdu)?,
        context.fits.cube,
        context.memory_limit_bytes,
    )?;
    let is_lumos_cfa = validate_cfa_image_header(path, &reader.hdus()[selected.index].header)?;
    let checksum_policy = if is_lumos_cfa {
        FitsChecksumPolicy::RequireValid
    } else {
        context.fits.checksum
    };
    let checksum = selection::verify_selected_checksum(
        &mut reader,
        selected.index,
        path,
        checksum_policy,
        context,
    )?;
    let quantization_sigma = read_quantization_sigma(&reader.hdus()[selected.index].header)
        .map_err(|source| fits_err(path, source))?;
    pixels::read_stream_hdu(&mut reader, selected, checksum, path, plan, context)?
        .into_cfa(path, quantization_sigma)
}

pub(crate) fn read_cfa_hdu(
    reader: &mut SliceReader<'_>,
    index: usize,
    path: &Path,
) -> Result<CfaImage, ImageError> {
    let context = LoadContext::default();
    let hdu = &reader.hdus()[index];
    let plan = plan::preflight_fits_image(
        path,
        FitsHduDescription::from_hdu(path, hdu)?,
        FitsCubeInterpretation::Reject,
        context.memory_limit_bytes,
    )?;
    let quantization_sigma = read_quantization_sigma(&reader.hdus()[index].header)
        .map_err(|source| fits_err(path, source))?;
    let header = reader.hdus()[index].header.clone();
    let selected = selection::selected_hdu(path, reader.hdus(), index)?;
    pixels::read_decoded_hdu(
        &header,
        plan,
        selected,
        FitsChecksumProvenance {
            datasum: FitsChecksumState::NotChecked,
            checksum: FitsChecksumState::NotChecked,
        },
        path,
        &context,
        |ranges| {
            reader
                .read_image_section(index, &ranges)
                .map(|image| image.physical_f32())
        },
    )?
    .into_cfa(path, quantization_sigma)
}

pub(crate) fn fits_cfa_frame_info(
    path: &Path,
    context: &LoadContext,
) -> Result<CfaFrameInfo, ImageError> {
    context.check_cancelled(path)?;
    let file = File::open(path).map_err(|source| ImageError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    let reader = FitsReader::open(file).map_err(|source| fits_err(path, source))?;
    context.check_cancelled(path)?;
    validate_cfa_container_format(path, reader.hdus().first().map(|hdu| &hdu.header))?;
    let selected = selection::select_image_hdu(path, reader.hdus(), &context.fits.hdu)?;
    let hdu = &reader.hdus()[selected.index];
    validate_cfa_image_header(path, &hdu.header)?;
    let dimensions = plan::preflight_fits_image(
        path,
        FitsHduDescription::from_hdu(path, hdu)?,
        context.fits.cube,
        context.memory_limit_bytes,
    )?
    .dimensions;
    if !dimensions.is_grayscale() {
        return Err(fits_unsupported(
            path,
            "scientific CFA input must have exactly one image plane",
        ));
    }
    let cfa_type = read_cfa_from_headers(&hdu.header)
        .map_err(|source| fits_err(path, source))?
        .ok_or_else(|| {
            fits_unsupported(
                path,
                "scientific CFA FITS input is missing validated CFA pattern metadata",
            )
        })?;
    Ok(CfaFrameInfo {
        dimensions,
        demosaic: cfa_type.demosaic_kind(),
    })
}

#[cfg(test)]
mod tests;
