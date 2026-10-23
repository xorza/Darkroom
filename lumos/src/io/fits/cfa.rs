use std::path::Path;

use common::fs;
use fits_well::FitsWriter;
use fits_well::header::Header;

use crate::image::cfa::cfa_image::CfaImage;
use crate::io::error::WriteError;
use crate::io::fits::error::{FitsLoadError, FitsRefusal};
use crate::io::fits::flags_extension;
use crate::io::fits::gain_extension;
use crate::io::fits::linear::LINEAR_FITS_FORMAT;
use crate::io::fits::metadata;
use crate::io::fits::metadata::{FORMAT_KEYWORD, VERSION_KEYWORD, domain_keywords};
use crate::io::fits::plane_hdu::{FloatPlanes, PlaneHdu};

pub(super) const CFA_FITS_FORMAT: &str = "CFAIMAGE";
/// 5: every flag is kept, in a `LUMFLAGS` extension, and the header records each part calibration
/// removed (`LUMCALB`, `LUMCALD`, `LUMCALF`) and what its dark was compared on (`LUMDK*`). A file
/// of an earlier version is refused, so a master cached by one, whose record is missing, is rebuilt
/// instead.
pub(super) const CFA_FITS_VERSION: i64 = 5;

#[derive(Debug, Clone, Copy)]
pub(crate) struct CfaFitsHduMetadata<'a> {
    pub(crate) extname: Option<&'a str>,
    pub(crate) image_type: Option<&'a str>,
    pub(crate) prepared: bool,
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

pub(super) fn validate_image_header(image: &Header) -> Result<bool, FitsLoadError> {
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
    let primary = hdu(
        image,
        CfaFitsHduMetadata {
            extname: None,
            image_type: image.metadata.header.image_type.as_deref(),
            prepared: false,
        },
    )?;
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

/// The HDU of `cfa`, with the per-HDU `hdu_metadata`: its mosaic, `NO_DATA` backed out as NaN, the
/// standard's own blank, which every FITS reader understands. Without it the samples under those
/// pixels — a decoder fill, or the same-color median `CfaImage::repair_nulls` put there — would
/// reload as measurements in a reader that does not know the `LUMFLAGS` extension.
pub(crate) fn hdu<'a>(
    cfa: &'a CfaImage,
    hdu_metadata: CfaFitsHduMetadata<'_>,
) -> Result<PlaneHdu<FloatPlanes<'a>>, WriteError> {
    let mut header = Header::new();
    header
        .set(FORMAT_KEYWORD, CFA_FITS_FORMAT)?
        .set(VERSION_KEYWORD, CFA_FITS_VERSION)?;
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
    Ok(PlaneHdu {
        header,
        shape: [cfa.data().width(), cfa.data().height()]
            .into_iter()
            .collect(),
        scaling: domain_keywords::scaling(&cfa.metadata),
        source: FloatPlanes::blanking([cfa.data().pixels()].into_iter().collect(), cfa.flags()),
    })
}
