use std::fs::File;
use std::path::Path;

use fits_well::io::{ChecksumReport, ChecksumStatus, Hdu, StreamReader};

use crate::io::image::error::ImageError;
use crate::io::image::fits::error::{fits_err, fits_unsupported};
use crate::io::image::fits::metadata::read_text;
use crate::io::image::fits::options::{FitsChecksumPolicy, FitsHduSelector};
use crate::io::image::fits::provenance::{
    FitsChecksumProvenance, FitsChecksumState, FitsHduProvenance,
};
use crate::io::image::load_context::LoadContext;

pub(super) fn selected_hdu(
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

pub(super) fn select_image_hdu(
    path: &Path,
    hdus: &[Hdu],
    selector: &FitsHduSelector,
) -> Result<FitsHduProvenance, ImageError> {
    let selected = match selector {
        FitsHduSelector::Auto => {
            let images: Vec<usize> = hdus
                .iter()
                .enumerate()
                .filter(|(_, hdu)| hdu.is_image())
                .map(|(index, _)| index)
                .collect();
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
                if hdu
                    .matches_extension(extname, *extver, None)
                    .map_err(|source| fits_err(path, source))?
                {
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
    if !hdus[selected.index].is_image() {
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

pub(super) fn verify_selected_checksum(
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
