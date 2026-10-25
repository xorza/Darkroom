//! [`Decoder`]: how the frames of a set are read from their files, as one image type; and what
//! identifies a decode: which decoder ran, at which version, under which options — what any record
//! of a decoded frame is keyed on, the decode cache's and a calibration master's alike.

pub(crate) mod cfa_decoder;
pub(crate) mod decode_params;
pub(crate) mod decode_pins;
pub(crate) mod decoder_kind;
pub(crate) mod fnv1a;
pub(crate) mod linear_decoder;

use std::fmt::Debug;
use std::path::Path;

use crate::image::flat_gain::shared_gains::SharedGains;
use crate::image::stackable_image::StackableImage;
use crate::io::decoder::decoder_kind::DecoderKind;
use crate::io::error::ImageError;
use crate::io::frame_info::FrameInfo;
use crate::io::load_ctx::LoadCtx;

/// How a frame set's files decode, as one image type: what a run generic over its frames' type
/// reads them through.
pub(crate) trait Decoder {
    type Image: StackableImage;

    /// What a header read leaves in hand that the load then takes rather than read again: a
    /// camera RAW's file, read whole for LibRaw to parse its header, or a FITS file's reader, with
    /// the HDU list it scanned and a compressed image's tile index.
    type Read: Send + Debug;

    /// The decoder [`Self::load`] runs, which a kept cache records beside the planes it wrote.
    const KIND: DecoderKind;

    /// The frame of `path` under `ctx`, the flat gain its file stores shared through `gains`.
    fn load(path: &Path, ctx: &LoadCtx, gains: &SharedGains) -> Result<Self::Image, ImageError>;

    /// The bytes `read` holds.
    fn read_len(read: &Self::Read) -> usize;

    /// [`Self::load`] of what a [`Self::peek`] of `path` read.
    fn load_read(
        read: Self::Read,
        path: &Path,
        ctx: &LoadCtx,
        gains: &SharedGains,
    ) -> Result<Self::Image, ImageError>;

    /// What a header alone settles about a frame, for a format that can answer without decoding.
    /// `None` when it cannot, or the header does not read, which leaves the caller to decode the
    /// frame and read the answer off it; the decode then reports what the header read did.
    fn peek(path: &Path, ctx: &LoadCtx) -> Option<Peek<Self::Read>>;
}

/// What a header read settles about a frame, and what it read that the frame's load takes.
#[derive(Debug)]
pub(crate) struct Peek<R> {
    pub(crate) info: FrameInfo,
    pub(crate) read: R,
}

impl<R> Peek<R> {
    /// This peek, with what it read wrapped as `wrap` makes it.
    pub(crate) fn map<S>(self, wrap: impl FnOnce(R) -> S) -> Peek<S> {
        Peek {
            info: self.info,
            read: wrap(self.read),
        }
    }
}
