//! The file formats: FITS, camera RAW and the formats imaginarium reads, decoded into the
//! [`image`](crate::image) types and written back from them.

pub(crate) mod error;
pub(crate) mod fits;
pub(crate) mod input_format;
pub(crate) mod load_context;
pub(crate) mod raw;
pub(crate) mod standard;

use imaginarium::SUPPORTED_EXTENSIONS;

use crate::io::input_format::FITS_EXTENSIONS;
use crate::io::raw::RAW_EXTENSIONS;

/// Every file extension accepted by
/// [`PreviewImage::from_file`](crate::image::preview_image::PreviewImage::from_file): FITS, camera
/// RAW, then imaginarium's formats, in the order its loader tries them.
pub const PREVIEW_IMAGE_EXTENSIONS: &[&str] = &PREVIEW_EXTENSION_TABLE;

const PREVIEW_EXTENSION_TABLE: [&str;
    FITS_EXTENSIONS.len() + RAW_EXTENSIONS.len() + SUPPORTED_EXTENSIONS.len()] = {
    let lists = [FITS_EXTENSIONS, RAW_EXTENSIONS, &SUPPORTED_EXTENSIONS];
    let mut table = [""; FITS_EXTENSIONS.len() + RAW_EXTENSIONS.len() + SUPPORTED_EXTENSIONS.len()];
    let mut next = 0;
    let mut list = 0;
    while list < lists.len() {
        let mut index = 0;
        while index < lists[list].len() {
            table[next] = lists[list][index];
            next += 1;
            index += 1;
        }
        list += 1;
    }
    table
};

#[cfg(test)]
mod tests;
