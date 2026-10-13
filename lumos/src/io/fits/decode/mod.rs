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

use crate::error::Cancelled;
use crate::frame::frame_peek::FramePeek;
use crate::image::cfa::cfa_frame_info::CfaFrameInfo;
use crate::image::cfa::cfa_image::CfaImage;
use crate::image::cfa::cfa_type::CfaType;
use crate::io::error::{FitsRefusal, ImageError, ScientificRefusal};
use crate::io::fits::decode::selected_fits::SelectedFits;

use crate::image::image_metadata::ImageMetadata;
use crate::image::image_provenance::ColorProvenance;
use crate::image::linear_image::LinearImage;
use crate::image::linear_pixels::LinearPixels;
use crate::image::pixel_flags::PixelFlags;
use crate::io::fits::metadata;
use crate::io::load_context::LoadContext;

mod hdu_sections;
pub(crate) mod open_fits;
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
                ScientificRefusal::MosaicFits,
            ));
        }

        check_gain_channels(path, &self.metadata, self.pixels.dimensions().channels())?;
        Ok(LinearImage {
            metadata: self.metadata,
            pixels: self.pixels,
            flags: self.flags,
        })
    }

    fn into_cfa(self, path: &Path) -> Result<CfaImage, ImageError> {
        let dimensions = self.pixels.dimensions();
        if !dimensions.is_grayscale() {
            return Err(ImageError::fits_unsupported(
                path,
                FitsRefusal::CfaPlanes {
                    planes: dimensions.channels(),
                },
            ));
        }
        let Some(cfa_type) = self.cfa_type else {
            return Err(ImageError::fits_unsupported(
                path,
                FitsRefusal::MissingCfaPattern,
            ));
        };

        check_gain_channels(path, &self.metadata, cfa_type.num_colors())?;
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

/// Refuse a stored flat gain whose grids are not one per colour of the mosaic, or per channel of
/// the image, that `metadata` describes: `channels` of them.
fn check_gain_channels(
    path: &Path,
    metadata: &ImageMetadata,
    channels: usize,
) -> Result<(), ImageError> {
    match metadata.flat_gain.as_deref() {
        Some(gain) if gain.channels() != channels => Err(ImageError::fits_unsupported(
            path,
            FitsRefusal::GainChannels {
                grids: gain.channels(),
                channels,
            },
        )),
        _ => Ok(()),
    }
}

/// Load an already-linear astronomical image from a FITS file.
pub(crate) fn load_linear(path: &Path, context: &LoadContext) -> Result<LinearImage, ImageError> {
    read_selected_image(path, context)?.into_linear(path)
}

pub(crate) fn load_preview(path: &Path, context: &LoadContext) -> Result<LinearImage, ImageError> {
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

pub(crate) fn load_cfa(path: &Path, context: &LoadContext) -> Result<CfaImage, ImageError> {
    SelectedFits::open(path, context)?
        .read(path, context)?
        .into_cfa(path)
}

pub(crate) fn cfa_frame_info(
    path: &Path,
    context: &LoadContext,
) -> Result<CfaFrameInfo, ImageError> {
    let selected = SelectedFits::open(path, context)?;
    let dimensions = selected.plan.dimensions;
    if !dimensions.is_grayscale() {
        return Err(ImageError::fits_unsupported(
            path,
            FitsRefusal::CfaPlanes {
                planes: dimensions.channels(),
            },
        ));
    }
    let cfa_type = metadata::read_cfa(
        selected.header(),
        dimensions.height(),
        context.fits.unstated_bayer_pattern,
    )
    .map_err(|source| ImageError::fits(path, source))?
    .ok_or_else(|| ImageError::fits_unsupported(path, FitsRefusal::MissingCfaPattern))?;
    Ok(CfaFrameInfo {
        dimensions,
        cfa_type,
        decoder_bytes: usize::try_from(selected.beside_frame_bytes())
            .expect("a decode the plan admitted fits the address space"),
    })
}

/// What a linear FITS image of `path` is worth to a memory estimate, from its header alone: the
/// dimensions it decodes to and what its decode holds beside them, by the same selection, cube
/// reading and limits [`load_linear`] applies, without reading a sample. A header that
/// declares a mosaic is refused, as the load refuses it.
pub(crate) fn linear_peek(path: &Path, context: &LoadContext) -> Result<FramePeek, ImageError> {
    let selected = SelectedFits::open(path, context)?;
    let dimensions = selected.plan.dimensions;
    let cfa_type = metadata::read_cfa(
        selected.header(),
        dimensions.height(),
        context.fits.unstated_bayer_pattern,
    )
    .map_err(|source| ImageError::fits(path, source))?;
    if cfa_type.is_some_and(|cfa_type| cfa_type != CfaType::Mono) {
        return Err(ImageError::scientific_rejection(
            path,
            ScientificRefusal::MosaicFits,
        ));
    }
    Ok(FramePeek {
        dimensions,
        decoder_bytes: usize::try_from(selected.beside_frame_bytes())
            .expect("a decode the plan admitted fits the address space"),
        mosaic: None,
    })
}

#[cfg(feature = "internals")]
pub(crate) mod bench;
#[cfg(test)]
mod tests;
