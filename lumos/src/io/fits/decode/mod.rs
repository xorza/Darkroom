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

use imaginarium::PlanarImage;

use crate::image::cfa::cfa_image::CfaImage;
use crate::image::cfa::cfa_type::CfaType;
use crate::image::flags::pixel_flags::PixelFlags;
use crate::image::flat_gain::FlatGain;
use crate::image::flat_gain::frame_gain::FrameGain;
use crate::image::linear_image::LinearImage;
use crate::image::metadata::image_metadata::ImageMetadata;
use crate::image::metadata::image_provenance::ColorProvenance;
use crate::io::fits::error::{FitsLoadError, FitsRefusal};

mod extension_chunks;
pub(crate) mod fits_file;
mod pixels;
mod plan;
pub(crate) mod selected_fits;
mod selection;

#[derive(Debug)]
pub(crate) struct DecodedFitsImage {
    metadata: ImageMetadata,
    pixels: PlanarImage,
    /// Where the HDU declared no measurement, or `None` when it declared none anywhere. Absent
    /// whenever [`NullPolicy::Reject`](crate::NullPolicy::Reject) is in force, which fails the load
    /// instead of reaching here.
    flags: Option<PixelFlags>,
    /// The gain the file's `LUMGAIN` extension stores, when it has one.
    flat_gain: Option<FrameGain>,
}

impl DecodedFitsImage {
    pub(crate) fn into_linear(self) -> Result<LinearImage, FitsLoadError> {
        check_gain_channels(
            self.flat_gain.as_deref(),
            self.pixels.desc().channel_count(),
        )?;
        let mut image = LinearImage::new(self.pixels, self.flags, self.metadata);
        image.set_flat_gain(self.flat_gain);
        Ok(image)
    }

    /// The image as the `cfa_type` mosaic its selection required.
    pub(crate) fn into_cfa(self, cfa_type: CfaType) -> Result<CfaImage, FitsLoadError> {
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
        let mut image = CfaImage::from_plane(pixels.into_mono(), cfa_type, metadata);
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

#[cfg(feature = "internals")]
pub(crate) mod bench;
#[cfg(test)]
mod tests;
