use fits_well::header::keywords;
use fits_well::io::{Hdu, HduSelector};

use crate::image::metadata::fits_transfer_provenance::FitsHduProvenance;
use crate::io::fits::error::{FitsLoadError, FitsRefusal};
use crate::io::fits::extension_claims::ExtensionClaims;
use crate::io::fits::fits_load_config::FitsHduSelector;
use crate::io::fits::metadata;

pub(super) fn selected_hdu(hdus: &[Hdu], idx: usize) -> Result<FitsHduProvenance, FitsLoadError> {
    let hdu = hdus.get(idx).ok_or_else(|| {
        FitsLoadError::from(FitsRefusal::HduOutOfRange {
            idx,
            hdus: hdus.len(),
        })
    })?;
    let extname = metadata::read_text(&hdu.header, keywords::EXTNAME)?;
    let extver = if extname.is_some() {
        Some(hdu.header.get_integer("EXTVER")?.unwrap_or(1))
    } else {
        None
    };
    Ok(FitsHduProvenance {
        idx,
        extname,
        extver,
    })
}

pub(super) fn select_image_hdu(
    hdus: &[Hdu],
    selector: &FitsHduSelector,
) -> Result<FitsHduProvenance, FitsLoadError> {
    let selected = match selector {
        FitsHduSelector::Auto => {
            let mut images = Vec::new();
            for (idx, hdu) in hdus.iter().enumerate() {
                if ExtensionClaims::holds_image(hdu)? {
                    images.push(idx);
                }
            }
            match images.as_slice() {
                [] => return Err(FitsLoadError::from(FitsRefusal::NoImageHdu)),
                [idx] => *idx,
                _ => {
                    return Err(FitsLoadError::from(FitsRefusal::SeveralImageHdus {
                        images: images.len(),
                    }));
                }
            }
        }
        FitsHduSelector::Idx(idx) => *idx,
        FitsHduSelector::Name { extname, extver } => {
            let mut matches = Vec::new();
            for (idx, hdu) in hdus.iter().enumerate() {
                let extension = HduSelector::Extension {
                    name: extname,
                    version: *extver,
                    level: None,
                };
                if hdu.matches(extension)? {
                    matches.push(idx);
                }
            }
            match matches.as_slice() {
                [] => {
                    return Err(FitsLoadError::from(FitsRefusal::NoMatchingHdu {
                        extname: extname.clone(),
                        extver: *extver,
                    }));
                }
                [idx] => *idx,
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
    let selected = selected_hdu(hdus, selected)?;
    if !hdus[selected.idx].is_image() {
        return Err(FitsLoadError::from(FitsRefusal::NotAnImage {
            hdu: selected.idx,
        }));
    }
    Ok(selected)
}
