pub(crate) mod calibration_state;
pub(crate) mod cfa;
pub(crate) mod error;
pub(crate) mod fits;
pub(crate) mod image_dimensions;
pub(crate) mod image_metadata;
pub(crate) mod image_provenance;
pub(crate) mod input_format;
pub(crate) mod linear;
pub(crate) mod linear_pixels;
pub(crate) mod load_context;
pub(crate) mod mosaic_noise;
pub(crate) mod pixel_flags;
pub(crate) mod preview_image;
pub(crate) mod sample_domain;
pub(crate) mod standard;

use imaginarium::SUPPORTED_EXTENSIONS;

use crate::io::image::input_format::FITS_EXTENSIONS;
use crate::io::raw::RAW_EXTENSIONS;

/// Every file extension accepted by [`preview_image::PreviewImage::from_file`]: FITS, camera RAW,
/// then imaginarium's formats, in the order its loader tries them.
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
