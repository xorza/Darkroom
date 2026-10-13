//! [`CfaFrameInfo`]: what a CFA frame is, read from its file without decoding it.

use std::path::Path;

use crate::image::cfa::cfa_type::CfaType;
use crate::image::image_dimensions::ImageDimensions;
use crate::io::error::{ImageError, ScientificRefusal};
use crate::io::fits::decode as fits_decode;
use crate::io::input_format::InputFormat;
use crate::io::load_context::LoadContext;
use crate::io::raw;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct CfaFrameInfo {
    pub(crate) dimensions: ImageDimensions,
    pub(crate) cfa_type: CfaType,
    /// What the decoder holds beside the frame while it makes it: a camera RAW's whole file, which
    /// LibRaw parses in place, and the raw buffer it unpacks into; nothing for a FITS file, which
    /// is streamed.
    pub(crate) decoder_bytes: usize,
}

impl CfaFrameInfo {
    pub(crate) fn from_file(path: &Path, context: &LoadContext) -> Result<Self, ImageError> {
        match InputFormat::of(path)? {
            InputFormat::Fits => fits_decode::cfa_frame_info(path, context),
            InputFormat::CameraRaw => raw::cfa_frame_info(path, context),
            InputFormat::Raster(_) => Err(ImageError::scientific_rejection(
                path,
                ScientificRefusal::RasterCfa,
            )),
        }
    }
}
