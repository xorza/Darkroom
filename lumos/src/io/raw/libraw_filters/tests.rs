use crate::image::cfa::cfa_pattern::CfaPattern;
use crate::image::cfa::cfa_type::CfaType;
use crate::image::cfa::mosaic::Mosaic;
use crate::image::error::XTransPatternError;
use crate::internals::cfa::XTRANS_PATTERN;
use crate::io::raw::libraw_filters::LibrawFilters;

/// LibRaw's `filters`, `colors` and `cdesc` classify a sensor: one color is mono whatever the
/// word says, `filters == 0` with three colors is a linear DNG or sRAW that LibRaw processes
/// itself, `9` is X-Trans with the pattern LibRaw keeps apart, a 2-row-periodic word is Bayer,
/// and any other word is an exotic CFA that LibRaw processes too. Four colors are no RGB mosaic
/// whatever the word: the Sony DSC-F828's `0x9c9c9c9c` reads R, E / G, B (emerald where a Bayer
/// phase has green) and the Nikon E950's CMYG `0x1e1e1e1e` reads as BGGR's phases; LibRaw
/// describes them `RGBE` and `CMYG`, with four colors.
#[test]
fn from_libraw_classifies_the_sensor() {
    let xtrans = XTRANS_PATTERN.rows();
    let classify = |filters, colors| {
        LibrawFilters {
            filters,
            colors,
            cdesc: *b"RGBG",
            xtrans,
        }
        .cfa_type()
        .unwrap()
    };
    assert_eq!(classify(0, 1), Some(CfaType::Mono));
    assert_eq!(classify(0x9494_9494, 1), Some(CfaType::Mono));
    assert_eq!(classify(0, 3), None);
    assert_eq!(
        classify(9, 3),
        Some(CfaType::Mosaic(Mosaic::XTrans(XTRANS_PATTERN)))
    );
    assert_eq!(
        classify(0x9494_9494, 3),
        Some(CfaType::Mosaic(Mosaic::Bayer(CfaPattern::Rggb)))
    );
    assert_eq!(
        classify(0x1616_1616, 3),
        Some(CfaType::Mosaic(Mosaic::Bayer(CfaPattern::Bggr)))
    );
    assert_eq!(classify(0x1234_5678, 3), None);
    for (filters, cdesc) in [(0x9c9c_9c9c, *b"RGBE"), (0x1e1e_1e1e, *b"CMYG")] {
        assert!(
            LibrawFilters::bayer(filters).is_some(),
            "{cdesc:?}: the word alone passes for Bayer"
        );
        assert_eq!(
            LibrawFilters {
                filters,
                colors: 4,
                cdesc,
                xtrans
            }
            .cfa_type()
            .unwrap(),
            None,
            "{cdesc:?}"
        );
        assert_eq!(
            LibrawFilters {
                filters,
                colors: 3,
                cdesc,
                xtrans
            }
            .cfa_type()
            .unwrap(),
            None,
            "{cdesc:?} described as three colors"
        );
    }
    // An X-Trans sensor whose layout is not one is a corrupt file, refused with the reason.
    let mut corrupt = xtrans;
    corrupt[0][0] = 3;
    assert!(matches!(
        LibrawFilters {
            filters: 9,
            colors: 3,
            cdesc: *b"RGBG",
            xtrans: corrupt
        }
        .cfa_type(),
        Err(XTransPatternError::Value {
            row: 0,
            col: 0,
            value: 3
        })
    ));
}

#[test]
fn from_filters_decodes_the_libraw_word_and_rejects_the_rest() {
    // Standard LibRaw encodings: two bits per position, the 2x2 block repeated across the word.
    assert_eq!(LibrawFilters::bayer(0x9494_9494), Some(CfaPattern::Rggb));
    assert_eq!(LibrawFilters::bayer(0x1616_1616), Some(CfaPattern::Bggr));
    assert_eq!(LibrawFilters::bayer(0x6161_6161), Some(CfaPattern::Grbg));
    assert_eq!(LibrawFilters::bayer(0x4949_4949), Some(CfaPattern::Gbrg));
    // filters == 0 is monochrome / no CFA; exotic patterns match nothing either.
    assert_eq!(LibrawFilters::bayer(0), None);
    assert_eq!(LibrawFilters::bayer(0x1234_5678), None);
    // RGGB in rows 0–5 but BGGR in rows 6–7: not 2-row periodic, whatever the first block says.
    assert_eq!(LibrawFilters::bayer(0x1694_9494), None);
}
