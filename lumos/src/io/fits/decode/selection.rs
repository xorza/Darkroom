use std::path::Path;

use fits_well::io::{ChecksumReport, ChecksumStatus, Hdu};

use crate::io::error::{FitsRefusal, ImageError};

use crate::io::fits::extension_claims::ExtensionClaims;
use crate::io::fits::fits_checksum_provenance::{FitsChecksumProvenance, FitsChecksumState};
use crate::io::fits::fits_load_options::{FitsChecksumPolicy, FitsHduSelector};
use crate::io::fits::fits_transfer_provenance::FitsHduProvenance;
use crate::io::fits::metadata;

pub(super) fn selected_hdu(
    path: &Path,
    hdus: &[Hdu],
    index: usize,
) -> Result<FitsHduProvenance, ImageError> {
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
    path: &Path,
    hdus: &[Hdu],
    selector: &FitsHduSelector,
) -> Result<FitsHduProvenance, ImageError> {
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
    let selected = selected_hdu(path, hdus, selected)?;
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

fn checksum_state(status: ChecksumStatus) -> FitsChecksumState {
    match status {
        ChecksumStatus::Absent => FitsChecksumState::Absent,
        ChecksumStatus::Unknown => FitsChecksumState::Unknown,
        ChecksumStatus::Valid => FitsChecksumState::Valid,
        ChecksumStatus::Invalid => {
            unreachable!("invalid FITS checksum is rejected before provenance is constructed")
        }
    }
}

fn checksum_provenance(report: ChecksumReport) -> FitsChecksumProvenance {
    FitsChecksumProvenance {
        datasum: checksum_state(report.datasum),
        checksum: checksum_state(report.checksum),
    }
}

/// HDU `index`'s checksum `report` under `policy`: an error where the policy refuses it, else the
/// provenance it gives. `policy` is not [`FitsChecksumPolicy::Ignore`], which reads nothing.
pub(super) fn judge_checksum(
    report: ChecksumReport,
    index: usize,
    path: &Path,
    policy: FitsChecksumPolicy,
) -> Result<FitsChecksumProvenance, ImageError> {
    match policy {
        FitsChecksumPolicy::Ignore => unreachable!("ignore policy returned before verification"),
        FitsChecksumPolicy::VerifyIfPresent => {
            if report.datasum == ChecksumStatus::Invalid
                || report.checksum == ChecksumStatus::Invalid
            {
                return Err(ImageError::fits_unsupported(
                    path,
                    FitsRefusal::InvalidChecksum { hdu: index, report },
                ));
            }
        }
        FitsChecksumPolicy::RequireValid => {
            if report.datasum != ChecksumStatus::Valid || report.checksum != ChecksumStatus::Valid {
                return Err(ImageError::fits_unsupported(
                    path,
                    FitsRefusal::ChecksumRequired { hdu: index, report },
                ));
            }
        }
    }
    Ok(checksum_provenance(report))
}
