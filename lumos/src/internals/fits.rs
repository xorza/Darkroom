//! FITS fixtures: an image written through fits-well, a file rewritten with edits, a file decoded
//! from its bytes, and the FITS record of a loaded one.

use std::io::Cursor;
use std::path::Path;

use common::fs;
use fits_well::header::Header;
use fits_well::image::Borrowed;
use fits_well::{FitsReader, FitsWriter};

use crate::image::cfa::cfa_image::CfaImage;
use crate::image::flat_gain::shared_gains::SharedGains;
use crate::image::linear_image::LinearImage;
use crate::image::metadata::fits_transfer_provenance::FitsTransferProvenance;
use crate::image::metadata::image_provenance::SourceProvenance;
use crate::io::error::ImageError;
use crate::io::fits::decode::fits_file::FitsFile;
use crate::io::fits::error::{FitsLoadError, FitsRefusal};
use crate::io::load_ctx::LoadCtx;

/// `image` as the bytes of a FITS file, with `header`'s cards when given.
pub(crate) fn fits_bytes(image: Borrowed<'_>, header: Option<&Header>) -> Vec<u8> {
    let mut writer = FitsWriter::new(Vec::new());
    writer.write_image(image, header).unwrap();
    writer.into_inner()
}

/// Write `image` to `path` as a FITS file, with `header`'s cards when given.
pub(crate) fn write_fits(path: &Path, image: Borrowed<'_>, header: Option<&Header>) {
    fs::write(path, fits_bytes(image, header)).unwrap();
}

/// The FITS file of `bytes`, open with the image `ctx`'s options select.
pub(crate) fn open_fits<'a>(
    bytes: &'a [u8],
    ctx: &LoadCtx,
) -> Result<FitsFile<Cursor<&'a [u8]>>, FitsLoadError> {
    FitsFile::new(Cursor::new(bytes), ctx)
}

/// The FITS file of `bytes` loaded as a linear image under `ctx`, sharing its gain with no other
/// frame.
pub(crate) fn load_linear(bytes: &[u8], ctx: &LoadCtx) -> Result<LinearImage, FitsLoadError> {
    open_fits(bytes, ctx)?.linear(ctx, &SharedGains::default())
}

/// The FITS file of `bytes` loaded as a sensor frame under `ctx`, sharing its gain with no other
/// frame.
pub(crate) fn load_cfa(bytes: &[u8], ctx: &LoadCtx) -> Result<CfaImage, FitsLoadError> {
    open_fits(bytes, ctx)?.cfa(ctx, &SharedGains::default())
}

/// `bytes`, a FITS file, rewritten with fresh checksums, each HDU's header and unpadded data
/// passed through `edit` first and dropped when it returns `false`: a file the loader trusts, so
/// a test reaches the checks behind the checksum.
pub(crate) fn rewritten_fits(
    bytes: &[u8],
    mut edit: impl FnMut(usize, &mut Header, &mut Vec<u8>) -> bool,
) -> Vec<u8> {
    let mut reader = FitsReader::from_bytes(bytes).unwrap();
    let mut writer = FitsWriter::new(Vec::new()).with_checksums();
    for idx in 0..reader.hdus().len() {
        let mut header = reader.hdus()[idx].header.clone();
        let mut data = reader.read_data_unit(idx).unwrap();
        if edit(idx, &mut header, &mut data) {
            writer.write_raw_hdu(&header, &data).unwrap();
        }
    }
    writer.into_inner()
}

/// Rewrite the FITS file at `path` as [`rewritten_fits`] does.
pub(crate) fn rewrite_fits(
    path: &Path,
    edit: impl FnMut(usize, &mut Header, &mut Vec<u8>) -> bool,
) {
    fs::write(path, rewritten_fits(&fs::read(path).unwrap(), edit)).unwrap();
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
