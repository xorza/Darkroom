//! A [`LinearImage`] as one FITS image HDU, a plane or an RGB cube, and the file that holds it with
//! its flags and flat gain beside it.

use std::path::Path;

use arrayvec::ArrayVec;
use common::fs;
use fits_well::FitsWriter;
use fits_well::header::Header;

use crate::image::cfa::cfa_type::CfaType;
use crate::image::linear_image::LinearImage;
use crate::io::error::WriteError;
use crate::io::fits::error::{FitsLoadError, FitsRefusal};
use crate::io::fits::flags_extension;
use crate::io::fits::gain_extension;
use crate::io::fits::metadata;
use crate::io::fits::metadata::{FORMAT_KEYWORD, VERSION_KEYWORD, domain_keywords};
use crate::io::fits::plane_hdu::{FloatPlanes, PlaneHdu};

pub(super) const LINEAR_FITS_FORMAT: &str = "LINIMAGE";
/// 1: the samples normalized with the domain's scale as `BSCALE`, an RGB image as a cube of three
/// planes, `NO_DATA` as NaN in every channel.
pub(super) const LINEAR_FITS_VERSION: i64 = 1;

/// The HDU of `linear`, the `cfa_type` mosaic when it is a stack of undemosaiced sensor frames:
/// its channels, `NO_DATA` as NaN in every one, as the CFA writer backs one out.
pub(crate) fn hdu(
    linear: &LinearImage,
    cfa_type: Option<CfaType>,
) -> Result<PlaneHdu<FloatPlanes<'_>>, WriteError> {
    let mut header = Header::new();
    header
        .set(FORMAT_KEYWORD, LINEAR_FITS_FORMAT)?
        .set(VERSION_KEYWORD, LINEAR_FITS_VERSION)?;
    metadata::write_image(
        &mut header,
        &linear.metadata,
        linear.metadata.header.image_type.as_deref(),
    )?;
    metadata::write_row_order(&mut header, &linear.metadata)?;
    if let Some(cfa_type) = cfa_type {
        metadata::write_cfa(&mut header, &linear.metadata, cfa_type)?;
    }
    let mut shape: ArrayVec<usize, 3> = [linear.width(), linear.height()].into_iter().collect();
    if linear.is_rgb() {
        shape.push(linear.channel_count());
    }
    Ok(PlaneHdu {
        header,
        shape,
        scaling: domain_keywords::scaling(&linear.metadata),
        source: FloatPlanes::blanking(
            (0..linear.channel_count())
                .map(|channel| linear.channel(channel).pixels())
                .collect(),
            linear.flags(),
        ),
    })
}

/// Write `image` to `path` as a checksummed floating-point FITS file: the primary HDU, then its
/// flags and its flat gain when it has them.
pub(crate) fn save(path: &Path, image: &LinearImage) -> Result<(), WriteError> {
    let primary = hdu(image, None)?;
    let flags = flags_extension::encode(image.flags(), None, 1)?;
    let gain = gain_extension::encode(image.flat_gain().map(|gain| &**gain), None)?;
    fs::publish(path, fs::PublicationMode::Durable, |file| {
        let mut writer = FitsWriter::new(&mut *file).with_checksums();
        primary.write(&mut writer)?;
        if let Some(flags) = &flags {
            flags.write(&mut writer)?;
        }
        if let Some(gain) = &gain {
            gain.write(&mut writer)?;
        }
        Ok(())
    })
}

/// Whether `image` is a Lumos linear image's header, refused when it is one of another version.
pub(crate) fn validate_image_header(image: &Header) -> Result<bool, FitsLoadError> {
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
