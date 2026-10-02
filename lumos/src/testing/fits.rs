//! FITS fixtures: an image written through fits-well, and the FITS record of a loaded one.

use std::fs::File;
use std::path::Path;

use fits_well::FitsWriter;
use fits_well::header::Header;
use fits_well::image::Image;

use crate::io::image::fits::provenance::FitsTransferProvenance;
use crate::io::image::linear::LinearImage;

/// Write `image` to `path` as a FITS file, with `header`'s cards when given.
pub(crate) fn write_fits(path: &Path, image: &Image, header: Option<&Header>) {
    let mut writer = FitsWriter::new(File::create(path).unwrap());
    writer.write_image(image, header).unwrap();
    writer.into_inner().sync_all().unwrap();
}

/// The FITS transfer record of an image the FITS decoder produced.
pub(crate) fn fits_transfer(image: &LinearImage) -> &FitsTransferProvenance {
    image
        .metadata
        .provenance
        .as_ref()
        .and_then(|provenance| provenance.transfer.fits())
        .expect("an image the FITS decoder produced")
}
