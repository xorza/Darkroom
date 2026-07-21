use std::fs::File;
use std::path::Path;

use fits_well::io::{ChecksumReport, ChecksumStatus, Hdu, HduKind, StreamReader};

use crate::io::image::LoadContext;
use crate::io::image::error::ImageError;
use crate::io::image::fits::error::{fits_err, fits_unsupported};
use crate::io::image::fits::metadata::read_text;
use crate::io::image::fits::options::{FitsChecksumPolicy, FitsHduSelector};
use crate::io::image::fits::provenance::{
    FitsChecksumProvenance, FitsChecksumState, FitsHduProvenance,
};

fn hdu_is_image(path: &Path, hdu: &Hdu) -> Result<bool, ImageError> {
    Ok(match hdu.kind {
        HduKind::Image | HduKind::CompressedImage => true,
        HduKind::Primary => {
            hdu.header
                .naxis()
                .map_err(|source| fits_err(path, source))?
                > 0
        }
        _ => false,
    })
}

pub(crate) fn selected_hdu(
    path: &Path,
    hdus: &[Hdu],
    index: usize,
) -> Result<FitsHduProvenance, ImageError> {
    let hdu = hdus.get(index).ok_or_else(|| {
        fits_unsupported(
            path,
            format!("HDU index {index} is out of range for {} HDUs", hdus.len()),
        )
    })?;
    let extname = read_text(&hdu.header, "EXTNAME").map_err(|source| fits_err(path, source))?;
    let extver = if extname.is_some() {
        Some(
            hdu.header
                .get_integer("EXTVER")
                .map_err(|source| fits_err(path, source))?
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

pub(crate) fn select_image_hdu(
    path: &Path,
    hdus: &[Hdu],
    selector: &FitsHduSelector,
) -> Result<FitsHduProvenance, ImageError> {
    let selected = match selector {
        FitsHduSelector::Auto => {
            let mut images = Vec::new();
            for (index, hdu) in hdus.iter().enumerate() {
                if hdu_is_image(path, hdu)? {
                    images.push(index);
                }
            }
            match images.as_slice() {
                [] => return Err(fits_unsupported(path, "no image HDU found")),
                [index] => *index,
                _ => {
                    return Err(fits_unsupported(
                        path,
                        format!(
                            "FITS file contains {} image HDUs; select one explicitly by index or EXTNAME/EXTVER",
                            images.len()
                        ),
                    ));
                }
            }
        }
        FitsHduSelector::Index(index) => *index,
        FitsHduSelector::Name { extname, extver } => {
            let mut matches = Vec::new();
            for (index, hdu) in hdus.iter().enumerate() {
                let Some(candidate) = hdu
                    .header
                    .get_text("EXTNAME")
                    .map_err(|source| fits_err(path, source))?
                else {
                    continue;
                };
                if !candidate.eq_ignore_ascii_case(extname) {
                    continue;
                }
                let candidate_version = hdu
                    .header
                    .get_integer("EXTVER")
                    .map_err(|source| fits_err(path, source))?
                    .unwrap_or(1);
                if extver.is_none_or(|version| version == candidate_version) {
                    matches.push(index);
                }
            }
            match matches.as_slice() {
                [] => {
                    return Err(fits_unsupported(
                        path,
                        format!("no HDU matches EXTNAME={extname:?}, EXTVER={extver:?}"),
                    ));
                }
                [index] => *index,
                _ => {
                    let reason = match extver {
                        Some(version) => format!(
                            "{} HDUs match EXTNAME={extname:?}, EXTVER={version}",
                            matches.len()
                        ),
                        None => format!(
                            "{} HDUs match EXTNAME={extname:?}; specify EXTVER",
                            matches.len()
                        ),
                    };
                    return Err(fits_unsupported(path, reason));
                }
            }
        }
    };
    let selected = selected_hdu(path, hdus, selected)?;
    if !hdu_is_image(path, &hdus[selected.index])? {
        return Err(fits_unsupported(
            path,
            format!("selected HDU {} is not an image", selected.index),
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

pub(crate) fn verify_selected_checksum(
    reader: &mut StreamReader<File>,
    index: usize,
    path: &Path,
    policy: FitsChecksumPolicy,
    context: &LoadContext,
) -> Result<FitsChecksumProvenance, ImageError> {
    if policy == FitsChecksumPolicy::Ignore {
        return Ok(FitsChecksumProvenance {
            datasum: FitsChecksumState::NotChecked,
            checksum: FitsChecksumState::NotChecked,
        });
    }
    context.check_cancelled(path)?;
    let report = reader
        .verify_checksum(index)
        .map_err(|source| fits_err(path, source))?;
    context.check_cancelled(path)?;
    match policy {
        FitsChecksumPolicy::Ignore => unreachable!("ignore policy returned before verification"),
        FitsChecksumPolicy::VerifyIfPresent => {
            if report.datasum == ChecksumStatus::Invalid
                || report.checksum == ChecksumStatus::Invalid
            {
                return Err(fits_unsupported(
                    path,
                    format!(
                        "selected HDU {index} has an invalid FITS checksum: DATASUM={:?}, CHECKSUM={:?}",
                        report.datasum, report.checksum
                    ),
                ));
            }
        }
        FitsChecksumPolicy::RequireValid => {
            if report.datasum != ChecksumStatus::Valid || report.checksum != ChecksumStatus::Valid {
                return Err(fits_unsupported(
                    path,
                    format!(
                        "selected HDU {index} requires valid DATASUM and CHECKSUM: DATASUM={:?}, CHECKSUM={:?}",
                        report.datasum, report.checksum
                    ),
                ));
            }
        }
    }
    Ok(checksum_provenance(report))
}
