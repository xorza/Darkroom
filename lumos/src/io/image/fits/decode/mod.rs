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
//! `DATAMAX`, and the ADC step read off the stored integers. The divisor is recorded in the image's
//! [`crate::SampleDomain`] so the physical value stays recoverable.
//!
//! The division does not make two frames mean the same thing — `BUNIT` is what says *what* they
//! measure, and a `Jy/beam` frame and a `count/s` one land on the same `[0, 1]` looking identical.
//! It is carried into [`crate::SampleDomain`] alongside the divisor rather than checked here: a
//! single frame in any unit is a legitimate load, and it is combining frames from two of them that
//! is not.

use std::path::Path;

use fits_well::io::SliceReader;

use crate::io::cancelled::Cancelled;
use crate::io::image::cfa::{CfaFrameInfo, CfaImage, CfaType};
use crate::io::image::error::ImageError;
use crate::io::image::fits::cfa::{CFA_FITS_FORMAT, validate_cfa_image_header};
use crate::io::image::fits::decode::hdu_sections::HduSections;
use crate::io::image::fits::decode::plan::FitsHduDescription;
use crate::io::image::fits::decode::selected_fits::SelectedFits;

use crate::io::image::fits::flags_extension::FlagsExtension;
use crate::io::image::fits::metadata::read_cfa_from_headers;
use crate::io::image::fits::options::FitsCubeInterpretation;
use crate::io::image::fits::provenance::FitsChecksumProvenance;
use crate::io::image::image_metadata::ImageMetadata;
use crate::io::image::image_provenance::ColorProvenance;
use crate::io::image::linear::LinearImage;
use crate::io::image::linear_pixels::LinearPixels;
use crate::io::image::load_context::LoadContext;
use crate::io::image::pixel_flags::PixelFlags;

mod hdu_sections;
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
    flags: Option<PixelFlags>,
}

impl DecodedFitsImage {
    fn into_linear(self, path: &Path) -> Result<LinearImage, ImageError> {
        // A mono sensor frame has no mosaic to demosaic, so it is linear data like any other
        // single-plane image.
        if self
            .cfa_type
            .is_some_and(|cfa_type| cfa_type != CfaType::Mono)
        {
            return Err(ImageError::scientific_rejection(
                path,
                "mosaic FITS must be loaded as CfaImage and calibrated before demosaicing",
            ));
        }

        Ok(LinearImage {
            metadata: self.metadata,
            pixels: self.pixels,
            flags: self.flags,
        })
    }

    fn into_cfa(self, path: &Path) -> Result<CfaImage, ImageError> {
        if !self.pixels.dimensions().is_grayscale() {
            return Err(ImageError::fits_unsupported(
                path,
                "scientific CFA input must have exactly one image plane",
            ));
        }
        let Some(cfa_type) = self.cfa_type else {
            return Err(ImageError::fits_unsupported(
                path,
                "scientific CFA FITS input is missing validated CFA pattern metadata",
            ));
        };

        let Self {
            mut metadata,
            pixels,
            flags,
            ..
        } = self;
        if let Some(provenance) = &mut metadata.provenance {
            provenance.color = ColorProvenance::SensorCfa;
        }
        Ok(CfaImage {
            data: pixels.into_l(),
            cfa_type,
            metadata,
            flags,
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
            .into_cfa(path)?
            .demosaic(context.xtrans_passes, &context.cancel)
            .map_err(|Cancelled| ImageError::cancelled(path))?)
    } else {
        decoded.into_linear(path)
    }
}

fn read_selected_image(path: &Path, context: &LoadContext) -> Result<DecodedFitsImage, ImageError> {
    SelectedFits::open(path, context)?.read(path, context)
}

pub(crate) fn load_cfa_fits(path: &Path, context: &LoadContext) -> Result<CfaImage, ImageError> {
    SelectedFits::open(path, context)?
        .read(path, context)?
        .into_cfa(path)
}

/// The Lumos CFA image in HDU `index` of a file already open in memory, with its flags extension,
/// under the caller's `context` — its cancellation, memory limit and float scale — with the
/// checksum state the caller verified for both. Held to the version check every file entry point
/// applies; an HDU that is not a Lumos CFA image is refused.
pub(crate) fn read_cfa_hdu(
    reader: &mut SliceReader<'_>,
    index: usize,
    path: &Path,
    context: &LoadContext,
    checksum: FitsChecksumProvenance,
) -> Result<CfaImage, ImageError> {
    context.check_cancelled(path)?;
    let hdu = &reader.hdus()[index];
    if !validate_cfa_image_header(path, &hdu.header)? {
        return Err(ImageError::fits_unsupported(
            path,
            format!("HDU {index} is not a Lumos {CFA_FITS_FORMAT} image"),
        ));
    }
    let plan = plan::preflight_fits_image(
        path,
        FitsHduDescription::from_hdu(path, hdu)?,
        FitsCubeInterpretation::Reject,
        context.fits.float_scale,
        context.memory_limit_bytes,
    )?;
    let size = plan.dimensions.size();
    let flags_hdu = FlagsExtension::locate(path, reader.hdus(), index, size)?;
    if flags_hdu.is_some() {
        plan.admit_flags_extension(path, context.memory_limit_bytes)?;
    }
    let header = reader.hdus()[index].header.clone();
    let selected = selection::selected_hdu(path, reader.hdus(), index)?;
    let mut decoded = pixels::read_decoded_hdu(
        &header,
        &plan,
        selected,
        path,
        context,
        &mut HduSections::new(reader, index),
        |_| Ok(checksum),
    )?;
    if let Some(flags_hdu) = flags_hdu {
        context.check_cancelled(path)?;
        let stored = reader
            .read_image(flags_hdu)
            .map_err(|source| ImageError::fits(path, source))?
            .decode();
        decoded.flags = FlagsExtension::join(path, stored, size, decoded.flags.as_ref())?;
    }
    decoded.into_cfa(path)
}

pub(crate) fn fits_cfa_frame_info(
    path: &Path,
    context: &LoadContext,
) -> Result<CfaFrameInfo, ImageError> {
    let selected = SelectedFits::open(path, context)?;
    let dimensions = selected.plan.dimensions;
    if !dimensions.is_grayscale() {
        return Err(ImageError::fits_unsupported(
            path,
            "scientific CFA input must have exactly one image plane",
        ));
    }
    let cfa_type = read_cfa_from_headers(
        selected.header(),
        dimensions.height(),
        context.fits.unstated_bayer_pattern,
    )
    .map_err(|source| ImageError::fits(path, source))?
    .ok_or_else(|| {
        ImageError::fits_unsupported(
            path,
            "scientific CFA FITS input is missing validated CFA pattern metadata",
        )
    })?;
    Ok(CfaFrameInfo {
        dimensions,
        cfa_type,
        may_carry_nulls: selected.plan.may_carry_nulls(),
        decoder_bytes: 0,
    })
}

#[cfg(all(test, feature = "bench"))]
mod bench;
#[cfg(test)]
mod tests;
