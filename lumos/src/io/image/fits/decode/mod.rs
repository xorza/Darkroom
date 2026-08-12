//! Decoding a FITS image HDU into the pipeline's linear `[0, 1]` domain.
//!
//! FITS stores physical values, not normalized ones, so an integer HDU is divided by the span its
//! own header declares — see [`plan::FitsDecodePlan::sample_divisor`]. That is what puts a FITS
//! frame in the same numeric domain as a RAW one, which every stage after `load` assumes without
//! being able to check. A floating-point HDU declares no span, so its `DATAMAX` decides: a
//! saturation level well above unity means ADU and is divided by the 16-bit full scale, and
//! anything else is taken as already normalized — which is what round-trips a Lumos-written master.
//!
//! Everything expressed in the file's sample units follows the samples through that division:
//! `DATAMAX`, a declared `QNTZSIG`, and the `BSCALE`-derived ADC step. The divisor is recorded as
//! [`crate::FitsTransferProvenance::physical_scale`] so the physical value stays recoverable.
//!
//! The division does not make two frames mean the same thing — `BUNIT` is what says *what* they
//! measure, and a `Jy/beam` frame and a `count/s` one land on the same `[0, 1]` looking identical.
//! It is carried into [`crate::SampleDomain`] alongside the divisor rather than checked here: a
//! single frame in any unit is a legitimate load, and it is combining frames from two of them that
//! is not.

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
use crate::io::image::fits::provenance::{FitsChecksumProvenance, FitsChecksumState};
use crate::io::image::image_metadata::{BitPix, ImageMetadata};
use crate::io::image::image_provenance::ColorProvenance;
use crate::io::image::linear::LinearImage;
use crate::io::image::linear_pixels::LinearPixels;
use crate::io::image::load_context::LoadContext;
use crate::io::image::standard::scientific_rejection;
use crate::io::raw::demosaic::DemosaicError;

mod pixels;
mod plan;
mod selection;

#[derive(Debug)]
struct DecodedFitsImage {
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

        // A declared QNTZSIG and a BSCALE-derived ADC step are both in the file's sample units, so
        // both follow the samples through the division the decoder already applied.
        let fits_transfer = self
            .metadata
            .provenance
            .as_ref()
            .and_then(|provenance| provenance.transfer.fits());
        let physical_scale = fits_transfer.map_or(1.0, |transfer| transfer.physical_scale);
        let quantization_sigma = declared_quantization_sigma
            .map(|sigma| sigma / physical_scale)
            .or_else(|| {
                let transfer = fits_transfer?;
                matches!(
                    self.metadata.bitpix,
                    BitPix::UInt8
                        | BitPix::Int16
                        | BitPix::UInt16
                        | BitPix::Int32
                        | BitPix::UInt32
                        | BitPix::Int64
                )
                .then(|| {
                    transfer.bscale.abs() as f32 / physical_scale * QUANTIZATION_SIGMA_PER_STEP
                })
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
        context.fits.float_scale,
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
        context.fits.float_scale,
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
        context.fits.float_scale,
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
        context.fits.float_scale,
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
