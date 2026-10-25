//! [`CodecFacts`]: what LibRaw reports of a file's codec at identify.

/// What LibRaw reports of a file's codec at identify.
#[derive(Debug, Clone, Copy)]
pub struct CodecFacts<'a> {
    /// The decoder's name as `libraw_get_decoder_info` reports it, without its terminator.
    pub name: &'a [u8],
    /// Whether the samples are floating-point.
    pub floating_point: bool,
    /// Whether a compressed Fuji RAF is lossless.
    pub fuji_lossless: bool,
    /// A CR3's coding, when the file has a CR3 track.
    pub crx: Option<CrxCoding>,
}

/// A CR3 track's coding, from its CMP1 header.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CrxCoding {
    /// 3 for the lossy plane-buffered variant.
    pub enc_type: i32,
    /// The wavelet levels: 0 codes the samples themselves, more codes quantized subbands.
    pub image_levels: i32,
}
