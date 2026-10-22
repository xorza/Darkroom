//! [`InputFormat`]: which decoder a file's extension routes it to, and the extensions each one
//! reads.

use std::path::Path;

use imaginarium::{FileFormat, SUPPORTED_EXTENSIONS};

use crate::io::error::ImageError;

/// The extensions of a FITS file.
const FITS_EXTENSIONS: &[&str] = &["fits", "fit"];

/// Camera-RAW extensions accepted by this decoder: the formats LibRaw decodes, as digiKam's
/// libkdcraw lists them, with Canon's CR3 that later LibRaw added. `.hdr`, which LibRaw reads for
/// Hasselblad, is left out: it names Radiance HDR images far more often. Sigma's X3F and GoPro's
/// GPR are left out too: LibRaw decodes them only with the X3F tools and the GPR SDK, which this
/// build does not include.
pub const RAW_EXTENSIONS: &[&str] = &[
    "3fr", "arw", "bay", "bmq", "cap", "cine", "cr2", "cr3", "crw", "cs1", "dc2", "dcr", "dng",
    "drf", "dsc", "erf", "fff", "ia", "iiq", "k25", "kc2", "kdc", "mdc", "mef", "mos", "mrw",
    "nef", "nrw", "orf", "pef", "ptx", "pxn", "qtk", "raf", "raw", "rdc", "rw2", "rwl", "rwz",
    "sr2", "srf", "srw", "sti",
];

/// Every file extension [`LoadCtx::preview`](crate::LoadCtx::preview) accepts: FITS, camera
/// RAW, then imaginarium's formats, in the order its loader tries them.
pub const PREVIEW_IMAGE_EXTENSIONS: &[&str] = &PREVIEW_EXTENSION_TABLE;

const PREVIEW_EXTENSION_TABLE: [&str;
    FITS_EXTENSIONS.len() + RAW_EXTENSIONS.len() + SUPPORTED_EXTENSIONS.len()] = {
    let lists = [FITS_EXTENSIONS, RAW_EXTENSIONS, &SUPPORTED_EXTENSIONS];
    let mut table = [""; FITS_EXTENSIONS.len() + RAW_EXTENSIONS.len() + SUPPORTED_EXTENSIONS.len()];
    let mut next = 0;
    let mut list = 0;
    while list < lists.len() {
        let mut idx = 0;
        while idx < lists[list].len() {
            table[next] = lists[list][idx];
            next += 1;
            idx += 1;
        }
        list += 1;
    }
    table
};

/// The container a file's extension names, which decides the decoder a load uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum InputFormat {
    Fits,
    CameraRaw,
    /// A raster imaginarium reads.
    Raster(FileFormat),
}

impl InputFormat {
    /// The format `path`'s extension names, in any case.
    pub(crate) fn of(path: &Path) -> Result<Self, ImageError> {
        let extension = path.extension().and_then(|extension| extension.to_str());
        let named_by = |list: &[&str]| {
            extension.is_some_and(|extension| {
                list.iter()
                    .any(|known| extension.eq_ignore_ascii_case(known))
            })
        };
        if named_by(FITS_EXTENSIONS) {
            Ok(Self::Fits)
        } else if named_by(RAW_EXTENSIONS) {
            Ok(Self::CameraRaw)
        } else {
            extension
                .and_then(FileFormat::from_extension)
                .map(Self::Raster)
                .ok_or_else(|| ImageError::UnsupportedFormat {
                    path: path.to_path_buf(),
                })
        }
    }
}

#[cfg(test)]
mod tests;
