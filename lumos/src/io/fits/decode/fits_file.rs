//! [`FitsFile`]: a FITS file open, its image selected and planned, for a load to read.

use std::fs::File;
use std::path::Path;

use fits_well::io::StreamReader;

use crate::image::flat_gain::shared_gains::SharedGains;
use crate::io::error::ImageError;
use crate::io::fits::decode::DecodedFitsImage;
use crate::io::fits::decode::selected_fits::{self, Checksum, SelectedFits};
use crate::io::fits::error::FitsLoadError;
use crate::io::load_ctx::LoadCtx;

/// A FITS file open, and the image of it the load's options select: what a header read leaves
/// for the frame's load, which reads on from the HDU list the reader scanned and the tile index of
/// a compressed image rather than open the file and read them again.
#[derive(Debug)]
pub(crate) struct FitsFile {
    reader: StreamReader<File>,
    pub(super) selected: SelectedFits,
}

impl FitsFile {
    /// The FITS file at `path`, open, with the image `ctx`'s options select.
    pub(super) fn open(path: &Path, ctx: &LoadCtx) -> Result<Self, ImageError> {
        let file = || {
            let mut reader = selected_fits::open(path, ctx)?;
            let selected = SelectedFits::select(&mut reader, ctx)?;
            Ok(Self { reader, selected })
        };
        file().map_err(|error| ImageError::fits_load(path, error))
    }

    /// The heap the file holds until its load takes it: what the reader keeps of the selected
    /// image, a compressed image's tile index.
    pub(crate) const fn held_bytes(&self) -> usize {
        self.selected.held_bytes
    }

    /// Decode the selected image — see [`SelectedFits::read`].
    pub(super) fn read(
        self,
        ctx: &LoadCtx,
        checksum: Checksum,
        gains: &SharedGains,
    ) -> Result<DecodedFitsImage, FitsLoadError> {
        let Self {
            mut reader,
            selected,
        } = self;
        selected.read(&mut reader, ctx, checksum, gains)
    }
}
