//! [`RawDecoder`]: the decoder LibRaw chose for a file, as far as lumos treats decoders apart.

/// The decoder LibRaw chose for a file at identify, with the facts of its codec lumos has to know.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum RawDecoder {
    /// Canon's CR3 codec: lossless, or with `lossy` its wavelet C-RAW, whose quantization steps are
    /// many ADU and whose codes never pass through LibRaw's `curve`.
    Crx { lossy: bool },
    /// Fuji's compressed RAF: lossless, or with `lossy` quantized through its own tables, again
    /// never through `curve`.
    FujiCompressed { lossy: bool },
    /// A Phase One IIQ, whose black LibRaw leaves in the raw buffer and subtracts only in its own
    /// processing, with its per-row and per-column corrections.
    PhaseOne,
    /// A floating-point DNG, which LibRaw converts to 16-bit integers on unpack: negatives clamped
    /// to 0, the rest truncated.
    FloatingPoint,
    /// A decoder that settles the mosaic's `filters` while it decodes, after the identify a header
    /// read stops at: the Raspberry Pi and Nokia sensor dumps, and Pentax's 4-shot.
    FiltersAtDecode,
    /// Any other decoder.
    Other,
}

/// What LibRaw reports of a file's codec at identify, as [`RawDecoder::classify`] reads it.
#[derive(Debug, Clone, Copy)]
pub(super) struct CodecFacts<'a> {
    /// The decoder's name as `libraw_get_decoder_info` reports it, without its terminator.
    pub(super) name: &'a [u8],
    /// Whether the samples are floating-point.
    pub(super) floating_point: bool,
    /// Whether a compressed Fuji RAF is lossless.
    pub(super) fuji_lossless: bool,
    /// A CR3's coding, when the file has a CR3 track.
    pub(super) crx: Option<CrxCoding>,
}

/// A CR3 track's coding, from its CMP1 header.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct CrxCoding {
    /// 3 for the lossy plane-buffered variant.
    pub(super) enc_type: i32,
    /// The wavelet levels: 0 codes the samples themselves, more codes quantized subbands.
    pub(super) image_levels: i32,
}

impl RawDecoder {
    /// The decoder `facts` describe. A CR3 is lossy wherever it is wavelet-coded or of the lossy
    /// type, and wherever its coding cannot be read: an unknown step is not one ADU.
    pub(super) fn classify(facts: CodecFacts<'_>) -> Self {
        if facts.floating_point {
            return Self::FloatingPoint;
        }
        match facts.name {
            b"crxLoadRaw()" => Self::Crx {
                lossy: facts
                    .crx
                    .is_none_or(|coding| coding.enc_type == 3 || coding.image_levels > 0),
            },
            b"fuji_compressed_load_raw()" => Self::FujiCompressed {
                lossy: !facts.fuji_lossless,
            },
            b"phase_one_load_raw()" | b"phase_one_load_raw_c()" | b"phase_one_load_raw_s()" => {
                Self::PhaseOne
            }
            b"rpi_load_raw8"
            | b"rpi_load_raw12"
            | b"rpi_load_raw14"
            | b"rpi_load_raw16"
            | b"nokia_load_raw()"
            | b"pentax_4shot_load_raw()" => Self::FiltersAtDecode,
            _ => Self::Other,
        }
    }

    /// Whether the codec quantizes past one ADU: its samples are no counts whose step is one.
    pub(super) const fn lossy(self) -> bool {
        matches!(
            self,
            Self::Crx { lossy: true } | Self::FujiCompressed { lossy: true }
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Each decoder name LibRaw reports names its case: a CR3 lossy when wavelet-coded, of type 3,
    /// or of a coding no track states; a compressed RAF lossy unless its header says lossless;
    /// any of the three Phase One loaders one case; the decoders that set `filters` as they run
    /// another; and a float file float whatever its decoder.
    #[test]
    fn a_decoder_classifies_by_its_name_and_codec() {
        let facts = |name: &'static [u8]| CodecFacts {
            name,
            floating_point: false,
            fuji_lossless: false,
            crx: None,
        };
        let crx = |enc_type, image_levels| CodecFacts {
            crx: Some(CrxCoding {
                enc_type,
                image_levels,
            }),
            ..facts(b"crxLoadRaw()")
        };
        for (case, decoder) in [
            (crx(0, 0), RawDecoder::Crx { lossy: false }),
            (crx(1, 0), RawDecoder::Crx { lossy: false }),
            (crx(0, 3), RawDecoder::Crx { lossy: true }),
            (crx(3, 0), RawDecoder::Crx { lossy: true }),
            (facts(b"crxLoadRaw()"), RawDecoder::Crx { lossy: true }),
            (
                CodecFacts {
                    fuji_lossless: true,
                    ..facts(b"fuji_compressed_load_raw()")
                },
                RawDecoder::FujiCompressed { lossy: false },
            ),
            (
                facts(b"fuji_compressed_load_raw()"),
                RawDecoder::FujiCompressed { lossy: true },
            ),
            (facts(b"phase_one_load_raw()"), RawDecoder::PhaseOne),
            (facts(b"phase_one_load_raw_c()"), RawDecoder::PhaseOne),
            (facts(b"phase_one_load_raw_s()"), RawDecoder::PhaseOne),
            (facts(b"rpi_load_raw12"), RawDecoder::FiltersAtDecode),
            (facts(b"nokia_load_raw()"), RawDecoder::FiltersAtDecode),
            (
                facts(b"pentax_4shot_load_raw()"),
                RawDecoder::FiltersAtDecode,
            ),
            (facts(b"lossless_dng_load_raw()"), RawDecoder::Other),
            (
                CodecFacts {
                    floating_point: true,
                    ..facts(b"deflate_dng_load_raw()")
                },
                RawDecoder::FloatingPoint,
            ),
        ] {
            assert_eq!(
                RawDecoder::classify(case),
                decoder,
                "{}",
                String::from_utf8_lossy(case.name)
            );
        }
        assert!(RawDecoder::Crx { lossy: true }.lossy());
        assert!(RawDecoder::FujiCompressed { lossy: true }.lossy());
        assert!(!RawDecoder::Crx { lossy: false }.lossy());
        assert!(!RawDecoder::Other.lossy());
    }
}
