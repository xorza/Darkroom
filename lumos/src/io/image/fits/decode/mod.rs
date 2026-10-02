//! Decoding a FITS image HDU into the pipeline's linear `[0, 1]` domain.
//!
//! FITS stores physical values, not normalized ones, so an integer HDU is divided by the span its
//! own header declares — see [`plan::FitsDecodePlan::sample_scale`]. That is what puts a FITS
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

use std::path::Path;

use fits_well::io::SliceReader;

use crate::io::cancelled::Cancelled;
use crate::io::image::cfa::{CfaFrameInfo, CfaImage, CfaType, QUANTIZATION_SIGMA_PER_STEP};
use crate::io::image::error::ImageError;
use crate::io::image::fits::decode::plan::FitsHduDescription;
use crate::io::image::fits::decode::selected_fits::SelectedFits;
use crate::io::image::fits::error::{fits_err, fits_unsupported};
use crate::io::image::fits::metadata::{read_cfa_from_headers, read_quantization_sigma};
use crate::io::image::fits::options::FitsCubeInterpretation;
use crate::io::image::fits::provenance::{FitsChecksumProvenance, FitsChecksumState};
use crate::io::image::image_metadata::ImageMetadata;
use crate::io::image::image_provenance::ColorProvenance;
use crate::io::image::linear::LinearImage;
use crate::io::image::linear_pixels::LinearPixels;
use crate::io::image::load_context::LoadContext;
use crate::io::image::null_mask::NullMask;
use crate::io::image::standard::scientific_rejection;

mod pixels;
mod plan;
mod selected_fits;
mod selection;

#[derive(Debug)]
struct DecodedFitsImage {
    metadata: ImageMetadata,
    /// The pattern the header declares, which makes the image a mosaic to load as a `CfaImage`.
    cfa_type: Option<CfaType>,
    pixels: LinearPixels,
    /// Where the HDU declared no measurement, or `None` when it declared none anywhere. Absent
    /// whenever [`FitsNullPolicy::Reject`](crate::FitsNullPolicy) is in force, which fails the load
    /// instead of reaching here.
    nulls: Option<NullMask>,
}

impl DecodedFitsImage {
    fn into_linear(self, path: &Path) -> Result<LinearImage, ImageError> {
        if self.cfa_type.is_some() {
            return Err(scientific_rejection(
                path,
                "mosaic FITS must be loaded as CfaImage and calibrated before demosaicing",
            ));
        }

        Ok(LinearImage {
            metadata: self.metadata,
            pixels: self.pixels,
            nulls: self.nulls,
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
        let Some(cfa_type) = self.cfa_type else {
            return Err(fits_unsupported(
                path,
                "scientific CFA FITS input is missing validated CFA pattern metadata",
            ));
        };

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
                self.metadata.sample_type?.is_integer().then(|| {
                    transfer.bscale.abs() as f32 / physical_scale * QUANTIZATION_SIGMA_PER_STEP
                })
            });
        let Self {
            mut metadata,
            pixels,
            nulls,
            ..
        } = self;
        if let Some(provenance) = &mut metadata.provenance {
            provenance.color = ColorProvenance::SensorCfa;
        }
        Ok(CfaImage {
            data: pixels.into_l(),
            cfa_type,
            metadata,
            quantization_sigma,
            nulls,
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
    if decoded.cfa_type.is_some() {
        Ok(decoded
            .into_cfa(path, None)?
            .demosaic(&context.cancel)
            .map_err(|Cancelled| ImageError::cancelled(path))?)
    } else {
        decoded.into_linear(path)
    }
}

fn read_selected_image(path: &Path, context: &LoadContext) -> Result<DecodedFitsImage, ImageError> {
    SelectedFits::open(path, context)?.read(path, context)
}

pub(crate) fn load_cfa_fits(path: &Path, context: &LoadContext) -> Result<CfaImage, ImageError> {
    let selected = SelectedFits::open(path, context)?;
    let quantization_sigma =
        read_quantization_sigma(selected.header()).map_err(|source| fits_err(path, source))?;
    selected
        .read(path, context)?
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
    let selected = SelectedFits::open(path, context)?;
    let dimensions = selected.plan.dimensions;
    if !dimensions.is_grayscale() {
        return Err(fits_unsupported(
            path,
            "scientific CFA input must have exactly one image plane",
        ));
    }
    let cfa_type = read_cfa_from_headers(selected.header(), context.fits.unstated_bayer_pattern)
        .map_err(|source| fits_err(path, source))?
        .ok_or_else(|| {
            fits_unsupported(
                path,
                "scientific CFA FITS input is missing validated CFA pattern metadata",
            )
        })?;
    Ok(CfaFrameInfo {
        dimensions,
        cfa_type,
        may_carry_nulls: selected.plan.may_carry_nulls(),
    })
}

#[cfg(test)]
mod tests;
