//! [`DecodeOptions`]: the options a decode's output depends on.

use common::SerdeFormat;

use crate::frame_store::decode_cache::fnv1a;
use crate::image::cfa::demosaic::xtrans::markesteijn::MarkesteijnPasses;
use crate::io::fits::fits_load_options::FitsLoadOptions;

/// The options a decode's output depends on, which a
/// [`LoadContext`](crate::io::load_context::LoadContext) and an
/// [`IngestConfig`](crate::IngestConfig) both carry, hashed one way for every cache keyed on them.
#[derive(Debug, Clone, Copy)]
pub(crate) struct DecodeOptions<'a> {
    pub(crate) fits: &'a FitsLoadOptions,
    pub(crate) xtrans_passes: MarkesteijnPasses,
}

impl DecodeOptions<'_> {
    /// FNV-1a of every field as bitcode, the checksum policy too: a frame decoded without a check
    /// would otherwise pass for one that a required checksum admitted.
    pub(crate) fn digest(self) -> u64 {
        let bytes = SerdeFormat::Bitcode
            .serialize(&(self.fits, self.xtrans_passes))
            .expect("decode options of plain enums and scalars always serialize");
        fnv1a::hash(fnv1a::OFFSET, &bytes)
    }
}
