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

use crate::image::cfa::cfa_image::CfaImage;
use crate::image::cfa::cfa_type::CfaType;
use crate::image::flat_gain::FlatGain;
use crate::image::flat_gain::frame_gain::FrameGain;
use crate::image::flat_gain::shared_gains::SharedGains;
use crate::image::image_metadata::ImageMetadata;
use crate::image::image_provenance::ColorProvenance;
use crate::image::linear_image::LinearImage;
use crate::image::linear_pixels::LinearPixels;
use crate::image::pixel_flags::PixelFlags;
use crate::io::error::{ImageError, ScientificRefusal};
use crate::io::fits::decode::selected_fits::{Checksum, SelectedFits};
use crate::io::fits::error::{FitsLoadError, FitsRefusal};
use crate::io::frame_info::{DecoderBytes, FrameInfo};
use crate::io::load_ctx::LoadCtx;
use crate::noise::mosaic_noise_measure::MosaicNoiseMeasure;

mod pixels;
mod plan;
pub(crate) mod selected_fits;
mod selection;

#[derive(Debug)]
pub(crate) struct DecodedFitsImage {
    metadata: ImageMetadata,
    pixels: LinearPixels,
    /// Where the HDU declared no measurement, or `None` when it declared none anywhere. Absent
    /// whenever [`NullPolicy::Reject`](crate::NullPolicy::Reject) is in force, which fails the load
    /// instead of reaching here.
    flags: Option<PixelFlags>,
    /// The gain the file's `LUMGAIN` extension stores, when it has one.
    flat_gain: Option<FrameGain>,
}

impl DecodedFitsImage {
    pub(super) fn into_linear(self) -> Result<LinearImage, FitsLoadError> {
        check_gain_channels(
            self.flat_gain.as_deref(),
            self.pixels.dimensions().channels(),
        )?;
        let mut image = LinearImage::new(self.pixels, self.flags, self.metadata);
        image.set_flat_gain(self.flat_gain);
        Ok(image)
    }

    /// The image as the `cfa_type` mosaic its selection required.
    pub(super) fn into_cfa(self, cfa_type: CfaType) -> Result<CfaImage, FitsLoadError> {
        check_gain_channels(self.flat_gain.as_deref(), cfa_type.color_count())?;
        let Self {
            mut metadata,
            pixels,
            flags,
            flat_gain,
        } = self;
        if let Some(provenance) = metadata.derived.provenance_mut() {
            provenance.color = ColorProvenance::SensorCfa;
        }
        let mut image = CfaImage::from_plane(pixels.into_l(), cfa_type, metadata);
        image.set_flags(flags);
        image.set_flat_gain(flat_gain);
        Ok(image)
    }
}

/// Refuse a stored flat gain whose grids are not one per color of the mosaic, or per channel of
/// the image: `channels` of them.
fn check_gain_channels(flat_gain: Option<&FlatGain>, channels: usize) -> Result<(), FitsLoadError> {
    match flat_gain {
        Some(gain) if gain.channels() != channels => Err(FitsRefusal::GainChannels {
            grids: gain.channels(),
            channels,
        }
        .into()),
        _ => Ok(()),
    }
}

/// Refuse, as a linear load of `path` does, an image whose header declares a mosaic: it loads as a
/// `CfaImage`, to be calibrated before it is demosaiced.
fn refuse_mosaic(path: &Path, selected: &SelectedFits<'_>) -> Result<(), ImageError> {
    if selected.is_mosaic() {
        return Err(ImageError::scientific_rejection(
            path,
            ScientificRefusal::MosaicFits,
        ));
    }
    Ok(())
}

/// Load an already-linear astronomical image from a FITS file, its flat gain shared through
/// `gains`.
pub(crate) fn load_linear(
    path: &Path,
    ctx: &LoadCtx,
    gains: &SharedGains,
) -> Result<LinearImage, ImageError> {
    let fits = |error| ImageError::fits_load(path, error);
    let mut reader = selected_fits::open(path, ctx).map_err(fits)?;
    let selected = SelectedFits::select(&mut reader, ctx).map_err(fits)?;
    refuse_mosaic(path, &selected)?;
    selected
        .read(ctx, Checksum::Judged, gains)
        .and_then(DecodedFitsImage::into_linear)
        .map_err(fits)
}

pub(crate) fn load_preview(path: &Path, ctx: &LoadCtx) -> Result<LinearImage, ImageError> {
    let preview = || {
        let mut reader = selected_fits::open(path, ctx)?;
        let selected = SelectedFits::select(&mut reader, ctx)?;
        if selected.cfa_type.is_none() {
            return selected
                .read(ctx, Checksum::Judged, &SharedGains::default())?
                .into_linear();
        }
        let info = selected.cfa_frame_info()?;
        let cfa = selected
            .read(ctx, Checksum::Judged, &SharedGains::default())?
            .into_cfa(info.cfa_type)?;
        let noise = MosaicNoiseMeasure::of(&cfa).map(MosaicNoiseMeasure::measure);
        Ok(cfa.demosaic(ctx.xtrans_passes, noise, &ctx.cancel)?)
    };
    preview().map_err(|error| ImageError::fits_load(path, error))
}

/// Load a sensor frame from a FITS file, its flat gain shared through `gains`.
pub(crate) fn load_cfa(
    path: &Path,
    ctx: &LoadCtx,
    gains: &SharedGains,
) -> Result<CfaImage, ImageError> {
    let cfa = || {
        let mut reader = selected_fits::open(path, ctx)?;
        let selected = SelectedFits::select(&mut reader, ctx)?;
        let cfa_type = selected.require_cfa()?;
        selected
            .read(ctx, Checksum::Judged, gains)?
            .into_cfa(cfa_type)
    };
    cfa().map_err(|error| ImageError::fits_load(path, error))
}

pub(crate) fn cfa_frame_info(path: &Path, ctx: &LoadCtx) -> Result<FrameInfo, ImageError> {
    let info = || {
        let mut reader = selected_fits::open(path, ctx)?;
        SelectedFits::select(&mut reader, ctx)?.cfa_frame_info()
    };
    info().map_err(|error| ImageError::fits_load(path, error))
}

/// What a linear FITS image of `path` is, from its header alone: the dimensions it decodes to and
/// what its decode holds beside them, by the same selection, cube reading and limits
/// [`load_linear`] applies, without reading a sample. A header that declares a mosaic is refused,
/// as the load refuses it.
pub(crate) fn linear_info(path: &Path, ctx: &LoadCtx) -> Result<FrameInfo, ImageError> {
    let fits = |error| ImageError::fits_load(path, error);
    let mut reader = selected_fits::open(path, ctx).map_err(fits)?;
    let selected = SelectedFits::select(&mut reader, ctx).map_err(fits)?;
    refuse_mosaic(path, &selected)?;
    Ok(FrameInfo {
        dimensions: selected.plan.dimensions,
        cfa_type: CfaType::Mono,
        decoder: DecoderBytes {
            source: 0,
            beside: usize::try_from(selected.beside_frame_bytes())
                .expect("a decode the plan admitted fits the address space"),
        },
        gain_bytes: selected.gain_bytes,
    })
}

#[cfg(feature = "internals")]
pub(crate) mod bench;
#[cfg(test)]
mod tests;
