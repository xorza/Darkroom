//! [`FrameInfo`]: what a frame is, read from its file's header without decoding it.

use crate::image::cfa::cfa_type::CfaType;
use crate::image::image_dimensions::ImageDimensions;

/// What a frame is, read from its header: its shape, its sensor's pattern — [`CfaType::Mono`] for
/// a frame of no filter, or of one already demosaiced — and what its decoder holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct FrameInfo {
    pub(crate) dimensions: ImageDimensions,
    pub(crate) cfa_type: CfaType,
    /// What the decoder holds beside the frame while it makes it: a camera RAW's whole file, which
    /// LibRaw parses in place, and the raw buffer it unpacks into; nothing for a FITS file, which
    /// is streamed.
    pub(crate) decoder_bytes: usize,
}
