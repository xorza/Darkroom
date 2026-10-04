use std::io;
use std::path::Path;

use common::file_utils;
use fits_well::FitsWriter;
use fits_well::header::Header;
use fits_well::image::Image;

use crate::io::image::cfa::CfaImage;
use crate::io::image::error::ImageError;
use crate::io::image::fits::error::fits_to_io;
use crate::io::image::fits::flags_extension::FlagsExtension;
use crate::io::image::fits::metadata::{write_cfa_metadata, write_image_metadata};
use crate::io::image::pixel_flags::QualityFlags;

pub(crate) const CFA_FITS_FORMAT: &str = "CFAIMAGE";
/// 3: every flag is kept, in a `LUMFLAGS` extension; a version-2 file kept only `NO_DATA`, so its
/// other flags are gone, and a master of that version is rebuilt instead.
pub(crate) const CFA_FITS_VERSION: i64 = 3;

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

pub(super) fn validate_cfa_container_format(
    path: &Path,
    primary: Option<&Header>,
) -> Result<(), ImageError> {
    if let Some(primary) = primary
        && let Some(format) = primary
            .get_text("LUMOSFMT")
            .map_err(|source| ImageError::fits(path, source))?
        && format != CFA_FITS_FORMAT
    {
        return Err(ImageError::fits_unsupported(
            path,
            format!("Lumos {format} FITS is not a standalone {CFA_FITS_FORMAT} image"),
        ));
    }
    Ok(())
}

pub(crate) fn validate_cfa_image_header(path: &Path, image: &Header) -> Result<bool, ImageError> {
    let is_lumos_cfa = image
        .get_text("LUMOSFMT")
        .map_err(|source| ImageError::fits(path, source))?
        .is_some_and(|format| format == CFA_FITS_FORMAT);
    if is_lumos_cfa {
        let version = image
            .get_integer("LUMOSVER")
            .map_err(|source| ImageError::fits(path, source))?;
        if version != Some(CFA_FITS_VERSION) {
            return Err(ImageError::fits_unsupported(
                path,
                format!(
                    "unsupported Lumos CFA FITS version {version:?}; expected {CFA_FITS_VERSION}"
                ),
            ));
        }
    }
    Ok(is_lumos_cfa)
}

pub(crate) fn save_cfa_fits(path: &Path, image: &CfaImage) -> io::Result<()> {
    let encoded = CfaFitsHdu::encode(
        image,
        CfaFitsHduMetadata {
            extname: None,
            image_type: image.metadata.image_type.as_deref(),
            prepared: false,
        },
    )?;
    let flags = FlagsExtension::encode(image.flags.as_ref(), None, 1)?;
    file_utils::publish(path, file_utils::PublicationMode::Durable, |file| {
        let mut writer = FitsWriter::new(&mut *file).with_checksums();
        writer
            .write_image(&encoded.image, Some(&encoded.header))
            .map_err(fits_to_io)?;
        if let Some(flags) = &flags {
            writer
                .write_image(&flags.image, Some(&flags.header))
                .map_err(fits_to_io)?;
        }
        Ok(())
    })
}

impl CfaFitsHdu {
    /// Build the image and header a CFA HDU writes, from `cfa` plus the per-HDU metadata.
    pub(crate) fn encode(cfa: &CfaImage, hdu_metadata: CfaFitsHduMetadata<'_>) -> io::Result<Self> {
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

        let mut samples = cfa.data.pixels().to_vec();
        if let Some(flags) = cfa
            .flags
            .as_ref()
            .filter(|flags| flags.contains(QualityFlags::NO_DATA))
        {
            // Back out as the standard's own flag, which every FITS reader understands. This is
            // written with a floating-point `BITPIX`, for which IEEE NaN *is* the blank. Without it
            // the samples under those pixels — a decoder fill, or the same-colour median
            // `CfaImage::repair_nulls` put there — would reload as measurements in a reader that
            // does not know the `LUMFLAGS` extension.
            let width = cfa.data.width();
            flags
                .mask_of(QualityFlags::NO_DATA)
                .for_each_set(|pos| samples[pos.y * width + pos.x] = f32::NAN);
        }
        let image =
            Image::new([cfa.data.width(), cfa.data.height()], samples).map_err(fits_to_io)?;
        Ok(Self { image, header })
    }
}
