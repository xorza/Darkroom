use std::path::Path;

use common::file_utils;
use fits_well::FitsWriter;
use fits_well::header::Header;
use fits_well::image::Image;

use crate::io::image::cfa::CfaImage;
use crate::io::image::error::ImageError;
use crate::io::image::fits::error::{fits_err, fits_to_io, fits_unsupported};
use crate::io::image::fits::metadata::{write_cfa_metadata, write_image_metadata};

pub(crate) const CFA_FITS_FORMAT: &str = "CFAIMAGE";
pub(crate) const CFA_FITS_VERSION: i64 = 1;

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

pub(crate) fn validate_cfa_container_format(
    path: &Path,
    primary: Option<&Header>,
) -> Result<(), ImageError> {
    if let Some(primary) = primary
        && let Some(format) = primary
            .get_text("LUMOSFMT")
            .map_err(|source| fits_err(path, source))?
        && format != CFA_FITS_FORMAT
    {
        return Err(fits_unsupported(
            path,
            format!("Lumos {format} FITS is not a standalone {CFA_FITS_FORMAT} image"),
        ));
    }
    Ok(())
}

pub(crate) fn validate_cfa_image_header(path: &Path, image: &Header) -> Result<bool, ImageError> {
    let is_lumos_cfa = image
        .get_text("LUMOSFMT")
        .map_err(|source| fits_err(path, source))?
        .is_some_and(|format| format == CFA_FITS_FORMAT);
    if is_lumos_cfa {
        let version = image
            .get_integer("LUMOSVER")
            .map_err(|source| fits_err(path, source))?;
        if version != Some(CFA_FITS_VERSION) {
            return Err(fits_unsupported(
                path,
                format!(
                    "unsupported Lumos CFA FITS version {version:?}; expected {CFA_FITS_VERSION}"
                ),
            ));
        }
    }
    Ok(is_lumos_cfa)
}

pub(crate) fn save_cfa_fits(path: &Path, image: &CfaImage) -> std::io::Result<()> {
    let encoded = encode_cfa_hdu(
        image,
        CfaFitsHduMetadata {
            extname: None,
            image_type: image.metadata.image_type.as_deref(),
            prepared: false,
        },
    )?;
    file_utils::publish(path, file_utils::PublicationMode::Durable, |file| {
        FitsWriter::new(&mut *file)
            .with_checksums()
            .write_image_with_header(&encoded.image, &encoded.header)
            .map_err(fits_to_io)
    })
}

pub(crate) fn encode_cfa_hdu(
    cfa: &CfaImage,
    hdu_metadata: CfaFitsHduMetadata<'_>,
) -> std::io::Result<CfaFitsHdu> {
    let mut header = Header::new();
    header
        .set("LUMOSFMT", CFA_FITS_FORMAT)
        .and_then(|header| header.set("LUMOSVER", CFA_FITS_VERSION))
        .map_err(fits_to_io)?;
    if let Some(extname) = hdu_metadata.extname {
        header.set("EXTNAME", extname).map_err(fits_to_io)?;
        header.set("LUMROLE", extname).map_err(fits_to_io)?;
    }
    if hdu_metadata.prepared {
        header.set("LUMPREP", true).map_err(fits_to_io)?;
    }
    write_image_metadata(&mut header, &cfa.metadata, hdu_metadata.image_type)
        .map_err(fits_to_io)?;
    write_cfa_metadata(&mut header, cfa).map_err(fits_to_io)?;

    let image = Image::new(
        [cfa.data.width(), cfa.data.height()],
        cfa.data.pixels().to_vec(),
    )
    .map_err(fits_to_io)?;
    Ok(CfaFitsHdu { image, header })
}
