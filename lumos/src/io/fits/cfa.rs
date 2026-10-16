use std::path::Path;

use std::sync::Arc;

use common::file_utils;
use fits_well::FitsWriter;
use fits_well::header::Header;
use fits_well::image::Image;

use crate::image::cfa::cfa_image::CfaImage;
use crate::image::quality_flags::QualityFlags;
use crate::io::error::WriteError;
use crate::io::fits::error::FitsLoadError;
use crate::io::fits::error::FitsRefusal;
use crate::io::fits::flags_extension::FlagsExtension;
use crate::io::fits::gain_extension::GainExtension;
use crate::io::fits::linear::LINEAR_FITS_FORMAT;
use crate::io::fits::metadata;
use crate::io::fits::metadata::domain_keywords;
use crate::io::fits::metadata::{FORMAT_KEYWORD, VERSION_KEYWORD};

pub(crate) const CFA_FITS_FORMAT: &str = "CFAIMAGE";
/// 5: every flag is kept, in a `LUMFLAGS` extension, and the header records each part calibration
/// removed (`LUMCALB`, `LUMCALD`, `LUMCALF`) and what its dark was compared on (`LUMDK*`). A file
/// of an earlier version is refused, so a master cached by one, whose record is missing, is rebuilt
/// instead.
pub(crate) const CFA_FITS_VERSION: i64 = 5;

#[derive(Debug, Clone, Copy)]
pub(crate) struct CfaFitsHduMetadata<'a> {
    pub(crate) extname: Option<&'a str>,
    pub(crate) image_type: Option<&'a str>,
    pub(crate) prepared: bool,
}

#[derive(Debug)]
pub(crate) struct CfaFitsHdu {
    pub(crate) image: Image,
    pub(crate) header: Header,
}

/// Refuse a file whose `primary` header names a Lumos container of other than one image, such as
/// a calibration bundle.
pub(super) fn validate_container_format(primary: Option<&Header>) -> Result<(), FitsLoadError> {
    if let Some(primary) = primary
        && let Some(format) = primary.get_text(FORMAT_KEYWORD)?
        && format != CFA_FITS_FORMAT
        && format != LINEAR_FITS_FORMAT
    {
        return Err(FitsLoadError::from(FitsRefusal::NotStandaloneImage {
            format: format.to_owned(),
        }));
    }
    Ok(())
}

pub(crate) fn validate_image_header(image: &Header) -> Result<bool, FitsLoadError> {
    let is_lumos_cfa = image
        .get_text(FORMAT_KEYWORD)?
        .is_some_and(|format| format == CFA_FITS_FORMAT);
    if is_lumos_cfa {
        let version = image.get_integer(VERSION_KEYWORD)?;
        if version != Some(CFA_FITS_VERSION) {
            return Err(FitsLoadError::from(FitsRefusal::CfaVersion { version }));
        }
    }
    Ok(is_lumos_cfa)
}

pub(crate) fn save(path: &Path, image: &CfaImage) -> Result<(), WriteError> {
    let encoded = CfaFitsHdu::encode(
        image,
        CfaFitsHduMetadata {
            extname: None,
            image_type: image.metadata.header.image_type.as_deref(),
            prepared: false,
        },
    )?;
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

impl CfaFitsHdu {
    /// Build the image and header a CFA HDU writes, from `cfa` plus the per-HDU metadata.
    pub(crate) fn encode(
        cfa: &CfaImage,
        hdu_metadata: CfaFitsHduMetadata<'_>,
    ) -> Result<Self, WriteError> {
        let mut header = Header::new();
        header
            .set(FORMAT_KEYWORD, CFA_FITS_FORMAT)
            .and_then(|header| header.set(VERSION_KEYWORD, CFA_FITS_VERSION))?;
        if let Some(extname) = hdu_metadata.extname {
            header.set("EXTNAME", extname)?;
            header.set("LUMROLE", extname)?;
        }
        if hdu_metadata.prepared {
            header.set("LUMPREP", true)?;
        }
        metadata::write_image(&mut header, &cfa.metadata, hdu_metadata.image_type)?;
        metadata::write_row_order(&mut header, &cfa.metadata)?;
        metadata::write_cfa(&mut header, &cfa.metadata, cfa.cfa_type())?;

        let mut samples = cfa.data().pixels().to_vec();
        if let Some(flags) = cfa
            .flags()
            .filter(|flags| flags.contains(QualityFlags::NO_DATA))
        {
            // Back out as the standard's own flag, which every FITS reader understands. This is
            // written with a floating-point `BITPIX`, for which IEEE NaN *is* the blank. Without it
            // the samples under those pixels — a decoder fill, or the same-color median
            // `CfaImage::repair_nulls` put there — would reload as measurements in a reader that
            // does not know the `LUMFLAGS` extension.
            let width = cfa.data().width();
            flags
                .mask_of(QualityFlags::NO_DATA)
                .for_each_set(|pos| samples[pos.y * width + pos.x] = f32::NAN);
        }
        let image = Image::new_scaled(
            [cfa.data().width(), cfa.data().height()],
            samples,
            domain_keywords::scaling(&cfa.metadata),
        )?;
        Ok(Self { image, header })
    }
}
