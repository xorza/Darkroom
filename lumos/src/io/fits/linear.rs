//! [`LinearFitsHdu`]: a [`LinearImage`] as one FITS image HDU, a plane or an RGB cube, and the file
//! that holds it with its flags and flat gain beside it.

use std::path::Path;
use std::sync::Arc;

use common::file_utils;
use fits_well::FitsWriter;
use fits_well::header::Header;
use fits_well::image::Image;

use crate::image::cfa::cfa_type::CfaType;
use crate::image::linear_image::LinearImage;
use crate::image::quality_flags::QualityFlags;
use crate::io::error::WriteError;
use crate::io::fits::error::{FitsLoadError, FitsRefusal};
use crate::io::fits::flags_extension::FlagsExtension;
use crate::io::fits::gain_extension::GainExtension;
use crate::io::fits::metadata;
use crate::io::fits::metadata::{FORMAT_KEYWORD, VERSION_KEYWORD, domain_keywords};

pub(super) const LINEAR_FITS_FORMAT: &str = "LINIMAGE";
/// 1: the samples normalized with the domain's scale as `BSCALE`, an RGB image as a cube of three
/// planes, `NO_DATA` as NaN in every channel.
pub(super) const LINEAR_FITS_VERSION: i64 = 1;

/// The image and header one linear image's HDU writes.
#[derive(Debug)]
pub(crate) struct LinearFitsHdu {
    pub(crate) image: Image,
    pub(crate) header: Header,
}

impl LinearFitsHdu {
    /// The HDU of `linear`, the `cfa_type` mosaic when it is a stack of undemosaiced sensor
    /// frames.
    pub(crate) fn encode(
        linear: &LinearImage,
        cfa_type: Option<CfaType>,
    ) -> Result<Self, WriteError> {
        let mut header = Header::new();
        header
            .set(FORMAT_KEYWORD, LINEAR_FITS_FORMAT)
            .and_then(|header| header.set(VERSION_KEYWORD, LINEAR_FITS_VERSION))?;
        metadata::write_image(
            &mut header,
            &linear.metadata,
            linear.metadata.header.image_type.as_deref(),
        )?;
        metadata::write_row_order(&mut header, &linear.metadata)?;
        if let Some(cfa_type) = cfa_type {
            metadata::write_cfa(&mut header, &linear.metadata, cfa_type)?;
        }

        let pixel_count = linear.pixel_count();
        let mut samples = Vec::with_capacity(linear.sample_count());
        for channel in 0..linear.channels() {
            samples.extend_from_slice(linear.channel(channel).pixels());
        }
        if let Some(flags) = linear
            .flags()
            .filter(|flags| flags.contains(QualityFlags::NO_DATA))
        {
            // The standard's own flag, in every channel, as the CFA writer backs one out: a reader
            // that does not know `LUMFLAGS` still sees no measurement there.
            let width = linear.width();
            flags.mask_of(QualityFlags::NO_DATA).for_each_set(|pos| {
                for plane in samples.chunks_exact_mut(pixel_count) {
                    plane[pos.y * width + pos.x] = f32::NAN;
                }
            });
        }
        let mut shape = vec![linear.width(), linear.height()];
        if linear.is_rgb() {
            shape.push(linear.channels());
        }
        let image = Image::new_scaled(shape, samples, domain_keywords::scaling(&linear.metadata))?;
        Ok(Self { image, header })
    }
}

/// Write `image` to `path` as a checksummed floating-point FITS file: the primary HDU, then its
/// flags and its flat gain when it has them.
pub(crate) fn save(path: &Path, image: &LinearImage) -> Result<(), WriteError> {
    let encoded = LinearFitsHdu::encode(image, None)?;
    let flags = FlagsExtension::encode(image.flags(), None, 1)?;
    let gain = GainExtension::encode(image.flat_gain().map(Arc::as_ref), None)?;
    file_utils::publish(path, file_utils::PublicationMode::Durable, |file| {
        let mut writer = FitsWriter::new(&mut *file).with_checksums();
        writer.write_image(&encoded.image, Some(&encoded.header))?;
        if let Some(flags) = &flags {
            writer.write_image(&flags.image, Some(&flags.header))?;
        }
        if let Some(gain) = &gain {
            writer.write_image(&gain.image, Some(&gain.header))?;
        }
        Ok(())
    })
}

/// Whether `image` is a Lumos linear image's header, refused when it is one of another version.
pub(super) fn validate_image_header(image: &Header) -> Result<bool, FitsLoadError> {
    let is_lumos_linear = image
        .get_text(FORMAT_KEYWORD)?
        .is_some_and(|format| format == LINEAR_FITS_FORMAT);
    if is_lumos_linear {
        let version = image.get_integer(VERSION_KEYWORD)?;
        if version != Some(LINEAR_FITS_VERSION) {
            return Err(FitsLoadError::from(FitsRefusal::LinearVersion { version }));
        }
    }
    Ok(is_lumos_linear)
}
