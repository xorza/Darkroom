use libraw::CrxCoding;

use crate::io::raw::raw_decoder::*;

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
