use crate::io::error::{FitsRefusal, ImageError};

use crate::io::fits::extension_claims::ExtensionClaims;
use crate::io::fits::fits_load_options::FitsHduSelector;
use crate::io::fits::fits_transfer_provenance::FitsHduProvenance;
use crate::io::fits::hdu_list::HduList;
use crate::io::fits::metadata;

pub(super) fn selected_hdu(
    file: HduList<'_>,
    index: usize,
) -> Result<FitsHduProvenance, ImageError> {
    let HduList { path, hdus } = file;
    let hdu = hdus.get(index).ok_or_else(|| {
        ImageError::fits_unsupported(
            path,
            FitsRefusal::HduOutOfRange {
                index,
                hdus: hdus.len(),
            },
        )
    })?;
    let extname = metadata::read_text(&hdu.header, "EXTNAME")
        .map_err(|source| ImageError::fits(path, source))?;
    let extver = if extname.is_some() {
        Some(
            hdu.header
                .get_integer("EXTVER")
                .map_err(|source| ImageError::fits(path, source))?
                .unwrap_or(1),
        )
    } else {
        None
    };
    Ok(FitsHduProvenance {
        index,
        extname,
        extver,
    })
}

pub(super) fn select_image_hdu(
    file: HduList<'_>,
    selector: &FitsHduSelector,
) -> Result<FitsHduProvenance, ImageError> {
    let HduList { path, hdus } = file;
    let selected = match selector {
        FitsHduSelector::Auto => {
            // A flags or gain extension describes another image, so it is no candidate of its own.
            let mut images = Vec::new();
            for (index, hdu) in hdus.iter().enumerate() {
                if hdu.is_image() && !ExtensionClaims::describes_another(path, hdu)? {
                    images.push(index);
                }
            }
            match images.as_slice() {
                [] => return Err(ImageError::fits_unsupported(path, FitsRefusal::NoImageHdu)),
                [index] => *index,
                _ => {
                    return Err(ImageError::fits_unsupported(
                        path,
                        FitsRefusal::SeveralImageHdus {
                            images: images.len(),
                        },
                    ));
                }
            }
        }
        FitsHduSelector::Index(index) => *index,
        FitsHduSelector::Name { extname, extver } => {
            let mut matches = Vec::new();
            for (index, hdu) in hdus.iter().enumerate() {
                if hdu
                    .matches_extension(extname, *extver, None)
                    .map_err(|source| ImageError::fits(path, source))?
                {
                    matches.push(index);
                }
            }
            match matches.as_slice() {
                [] => {
                    return Err(ImageError::fits_unsupported(
                        path,
                        FitsRefusal::NoMatchingHdu {
                            extname: extname.clone(),
                            extver: *extver,
                        },
                    ));
                }
                [index] => *index,
                _ => {
                    return Err(ImageError::fits_unsupported(
                        path,
                        FitsRefusal::SeveralMatchingHdus {
                            extname: extname.clone(),
                            extver: *extver,
                            matches: matches.len(),
                        },
                    ));
                }
            }
        }
    };
    let selected = selected_hdu(file, selected)?;
    if !hdus[selected.index].is_image() {
        return Err(ImageError::fits_unsupported(
            path,
            FitsRefusal::NotAnImage {
                hdu: selected.index,
            },
        ));
    }
    Ok(selected)
}
