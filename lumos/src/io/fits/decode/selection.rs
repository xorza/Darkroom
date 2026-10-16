use crate::io::fits::error::FitsLoadError;
use crate::io::fits::error::FitsRefusal;

use crate::image::fits_transfer_provenance::FitsHduProvenance;
use crate::io::fits::extension_claims::ExtensionClaims;
use crate::io::fits::fits_load_options::FitsHduSelector;
use crate::io::fits::metadata;
use fits_well::io::Hdu;

pub(super) fn selected_hdu(file: &[Hdu], index: usize) -> Result<FitsHduProvenance, FitsLoadError> {
    let hdus = file;
    let hdu = hdus.get(index).ok_or_else(|| {
        FitsLoadError::from(FitsRefusal::HduOutOfRange {
            index,
            hdus: hdus.len(),
        })
    })?;
    let extname = metadata::read_text(&hdu.header, "EXTNAME")?;
    let extver = if extname.is_some() {
        Some(hdu.header.get_integer("EXTVER")?.unwrap_or(1))
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
    file: &[Hdu],
    selector: &FitsHduSelector,
) -> Result<FitsHduProvenance, FitsLoadError> {
    let hdus = file;
    let selected = match selector {
        FitsHduSelector::Auto => {
            let mut images = Vec::new();
            for (index, hdu) in hdus.iter().enumerate() {
                if ExtensionClaims::holds_image(hdu)? {
                    images.push(index);
                }
            }
            match images.as_slice() {
                [] => return Err(FitsLoadError::from(FitsRefusal::NoImageHdu)),
                [index] => *index,
                _ => {
                    return Err(FitsLoadError::from(FitsRefusal::SeveralImageHdus {
                        images: images.len(),
                    }));
                }
            }
        }
        FitsHduSelector::Index(index) => *index,
        FitsHduSelector::Name { extname, extver } => {
            let mut matches = Vec::new();
            for (index, hdu) in hdus.iter().enumerate() {
                if hdu.matches_extension(extname, *extver, None)? {
                    matches.push(index);
                }
            }
            match matches.as_slice() {
                [] => {
                    return Err(FitsLoadError::from(FitsRefusal::NoMatchingHdu {
                        extname: extname.clone(),
                        extver: *extver,
                    }));
                }
                [index] => *index,
                _ => {
                    return Err(FitsLoadError::from(FitsRefusal::SeveralMatchingHdus {
                        extname: extname.clone(),
                        extver: *extver,
                        matches: matches.len(),
                    }));
                }
            }
        }
    };
    let selected = selected_hdu(file, selected)?;
    if !hdus[selected.index].is_image() {
        return Err(FitsLoadError::from(FitsRefusal::NotAnImage {
            hdu: selected.index,
        }));
    }
    Ok(selected)
}
