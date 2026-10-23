//! FITS fixtures: an image written through fits-well, a file rewritten with edits, and the FITS
//! record of a loaded one.

use std::path::Path;

use common::fs;
use fits_well::header::Header;
use fits_well::image::Borrowed;
use fits_well::{FitsReader, FitsWriter};

use crate::image::fits_transfer_provenance::FitsTransferProvenance;
use crate::image::image_provenance::SourceProvenance;
use crate::image::linear_image::LinearImage;
use crate::io::error::ImageError;
use crate::io::fits::error::{FitsLoadError, FitsRefusal};

/// Write `image` to `path` as a FITS file, with `header`'s cards when given.
pub(crate) fn write_fits(path: &Path, image: Borrowed<'_>, header: Option<&Header>) {
    let mut writer = FitsWriter::new(fs::create(path).unwrap());
    writer.write_image(image, header).unwrap();
    writer.into_inner().sync_all().unwrap();
}

/// Rewrite the FITS file at `path` with fresh checksums, each HDU's header and unpadded data
/// passed through `edit` first and dropped when it returns `false`: a file the loader trusts, so
/// a test reaches the checks behind the checksum.
pub(crate) fn rewrite_fits(
    path: &Path,
    mut edit: impl FnMut(usize, &mut Header, &mut Vec<u8>) -> bool,
) {
    let bytes = fs::read(path).unwrap();
    let mut reader = FitsReader::from_bytes(&bytes).unwrap();
    let mut writer = FitsWriter::new(Vec::new()).with_checksums();
    for idx in 0..reader.hdus().len() {
        let mut header = reader.hdus()[idx].header.clone();
        let mut data = reader.read_data_unit(idx).unwrap().into_data();
        if edit(idx, &mut header, &mut data) {
            writer.write_raw_hdu(&header, &data).unwrap();
        }
    }
    fs::write(path, writer.into_inner()).unwrap();
}

/// The FITS transfer record of an image the FITS decoder produced.
pub(crate) fn fits_transfer(image: &LinearImage) -> &FitsTransferProvenance {
    match image
        .metadata
        .derived
        .provenance()
        .map(|provenance| &provenance.source)
    {
        Some(SourceProvenance::Fits(transfer)) => transfer,
        other => panic!("expected an image the FITS decoder produced, got {other:?}"),
    }
}

/// An error that can hold a FITS refusal: the load's own, or the one a caller of an entry point
/// receives.
pub(crate) trait FitsFailure: std::fmt::Debug {
    fn refusal(self) -> Option<FitsRefusal>;
}

impl FitsFailure for ImageError {
    fn refusal(self) -> Option<FitsRefusal> {
        match self {
            Self::FitsUnsupported { reason, .. } => Some(reason),
            _ => None,
        }
    }
}

impl FitsFailure for FitsLoadError {
    fn refusal(self) -> Option<FitsRefusal> {
        match self {
            Self::Refused(reason) => Some(reason),
            _ => None,
        }
    }
}

/// Why `error` refused a FITS file; panics on any other error.
pub(crate) fn fits_refusal(error: impl FitsFailure) -> FitsRefusal {
    let shown = format!("{error:?}");
    error
        .refusal()
        .unwrap_or_else(|| panic!("expected an unsupported-FITS refusal, got {shown}"))
}
