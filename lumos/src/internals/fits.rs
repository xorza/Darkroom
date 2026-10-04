//! FITS fixtures: an image written through fits-well, a file rewritten with edits, and the FITS
//! record of a loaded one.

use std::fs;
use std::fs::File;
use std::path::Path;

use fits_well::header::Header;
use fits_well::image::Image;
use fits_well::{FitsReader, FitsWriter};

use crate::io::image::fits::provenance::FitsTransferProvenance;
use crate::io::image::image_provenance::TransferProvenance;
use crate::io::image::linear::LinearImage;

/// Write `image` to `path` as a FITS file, with `header`'s cards when given.
pub(crate) fn write_fits(path: &Path, image: &Image, header: Option<&Header>) {
    let mut writer = FitsWriter::new(File::create(path).unwrap());
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
    for index in 0..reader.hdus().len() {
        let mut header = reader.hdus()[index].header.clone();
        let mut data = reader.read_data_raw(index).unwrap().into_data();
        if edit(index, &mut header, &mut data) {
            writer.write_raw_hdu(&header, &data).unwrap();
        }
    }
    fs::write(path, writer.into_inner()).unwrap();
}

/// The FITS transfer record of an image the FITS decoder produced.
pub(crate) fn fits_transfer(image: &LinearImage) -> &FitsTransferProvenance {
    match image
        .metadata
        .provenance
        .as_ref()
        .map(|provenance| &provenance.transfer)
    {
        Some(TransferProvenance::FitsNormalized(transfer)) => transfer,
        other => panic!("expected an image the FITS decoder produced, got {other:?}"),
    }
}
