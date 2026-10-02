use fits_well::header::Header;

use crate::io::image::cfa::CfaType;
use fits_well::image::SampleType;

use crate::io::image::fits::metadata::{
    SOURCE_ROW_ORDER, parse_sexagesimal, read_bayer_cfa, read_declared_row_order, read_metadata,
    read_row_order,
};
use crate::io::image::image_provenance::RowOrder;
use crate::io::raw::demosaic::bayer::CfaPattern;

/// A minimal Bayer image header, with `ROWORDER` omitted when `roworder` is `None`.
fn bayer_header(bayerpat: &str, roworder: Option<&str>, height: i64) -> Header {
    let mut header = Header::new();
    header.set("BAYERPAT", bayerpat).unwrap();
    header.set("NAXIS2", height).unwrap();
    if let Some(roworder) = roworder {
        header.set("ROWORDER", roworder).unwrap();
    }
    header
}

fn pattern_of(header: &Header) -> CfaPattern {
    match read_bayer_cfa(header, true, None).unwrap() {
        Some(CfaType::Bayer(pattern)) => pattern,
        other => panic!("expected a Bayer pattern, got {other:?}"),
    }
}

#[test]
fn a_bottom_up_frame_flips_its_bayer_phase_only_when_its_height_is_even() {
    // `BAYERPAT` describes the top-down image and the rows arrive in file order, so file row
    // `f` holds the phase of displayed row `H - 1 - f`.
    //
    // H = 4: file row 0 is displayed row 3, which is phase 1 — the G/B row of an RGGB frame —
    // and file row 1 is displayed row 2, phase 0. Reading the file top to bottom therefore
    // gives GB then RG, which is GBRG.
    assert_eq!(
        pattern_of(&bayer_header("RGGB", Some("BOTTOM-UP"), 4)),
        CfaPattern::Gbrg
    );

    // H = 5: file row 0 is displayed row 4, which is phase 0 again, so the file reads RG then
    // GB and the declared pattern already describes it. Flipping here is what mis-debayered the
    // whole frame — and odd heights are real, LibRaw reporting 4015 for the EOS 1500D.
    assert_eq!(
        pattern_of(&bayer_header("RGGB", Some("BOTTOM-UP"), 4015)),
        CfaPattern::Rggb
    );

    // Top-down, and a header that declares no order at all, leave the pattern alone whatever
    // the parity — the height only matters because reversal is what moves the phases.
    for height in [4, 5] {
        assert_eq!(
            pattern_of(&bayer_header("RGGB", Some("TOP-DOWN"), height)),
            CfaPattern::Rggb,
            "top-down, height {height}"
        );
        assert_eq!(
            pattern_of(&bayer_header("RGGB", None, height)),
            CfaPattern::Rggb,
            "no ROWORDER, height {height}"
        );
    }
}

#[test]
fn an_odd_bayer_row_offset_composes_with_the_row_order_flip() {
    // `YBAYROFF` shifts the pattern's origin by a row, which is the same phase inversion the
    // row-order flip applies. On an even-height bottom-up frame the two compose back to the
    // declared pattern; on an odd-height one only the offset acts, so they no longer agree —
    // which is exactly the composition the parity fix changes.
    assert_eq!(
        pattern_of(
            bayer_header("RGGB", Some("BOTTOM-UP"), 4)
                .set("YBAYROFF", 1)
                .unwrap()
        ),
        CfaPattern::Rggb
    );
    assert_eq!(
        pattern_of(
            bayer_header("RGGB", Some("BOTTOM-UP"), 5)
                .set("YBAYROFF", 1)
                .unwrap()
        ),
        CfaPattern::Gbrg
    );
}

#[test]
fn row_order_is_read_from_the_header_and_defaults_to_top_down() {
    // Recorded so a set mixing the two can be named; nothing reorders rows on it.
    let mut header = Header::new();
    assert_eq!(read_row_order(&header).unwrap(), RowOrder::TopDown);

    for declared in ["BOTTOM-UP", "bottom-up", " BOTTOM-UP "] {
        header.set("ROWORDER", declared).unwrap();
        assert_eq!(
            read_row_order(&header).unwrap(),
            RowOrder::BottomUp,
            "{declared}"
        );
    }

    // Anything the keyword does not spell as bottom-up is read the way a writer that omits it
    // means — first row first.
    for declared in ["TOP-DOWN", "top-down", "anything else"] {
        header.set("ROWORDER", declared).unwrap();
        assert_eq!(
            read_row_order(&header).unwrap(),
            RowOrder::TopDown,
            "{declared}"
        );
    }
}

#[test]
fn a_lumos_written_header_separates_the_pattern_frame_from_the_sky_orientation() {
    // What this writer emits for a bottom-up source: the rows go out as it holds them, so
    // `ROWORDER` says the pattern applies to them as written, and `LUMROWO` carries the fact
    // that the sky is upside-down in them.
    let mut header = Header::new();
    header.set("BAYERPAT", "GBRG").unwrap();
    header.set("NAXIS2", 4).unwrap();
    header.set("ROWORDER", RowOrder::TopDown.keyword()).unwrap();
    header
        .set(SOURCE_ROW_ORDER, RowOrder::BottomUp.keyword())
        .unwrap();

    // The pattern is taken as written — flipping it again is what would mis-debayer the
    // reloaded frame, and is what reading `LUMROWO` here instead would have done.
    assert_eq!(read_declared_row_order(&header).unwrap(), RowOrder::TopDown);
    assert_eq!(pattern_of(&header), CfaPattern::Gbrg);

    // ...while the orientation the combine compares survives the trip, so an original and a
    // copy of it written by this crate are not read as mirrored views of each other.
    assert_eq!(read_row_order(&header).unwrap(), RowOrder::BottomUp);

    // A file from anyone else has only `ROWORDER`, and then the two answers coincide.
    let third_party = bayer_header("RGGB", Some(RowOrder::BottomUp.keyword()), 4);
    assert_eq!(
        read_declared_row_order(&third_party).unwrap(),
        RowOrder::BottomUp
    );
    assert_eq!(read_row_order(&third_party).unwrap(), RowOrder::BottomUp);
}

#[test]
fn a_bottom_up_frame_without_a_height_is_rejected_rather_than_guessed() {
    // The flip decision turns on the height's parity, so a header that declares none cannot be
    // resolved — and assuming either parity mis-debayers every frame that has the other.
    let mut header = Header::new();
    header.set("BAYERPAT", "RGGB").unwrap();
    header.set("ROWORDER", "BOTTOM-UP").unwrap();
    assert!(matches!(
        read_bayer_cfa(&header, true, None),
        Err(fits_well::FitsError::MissingKeyword { name: "NAXIS2" })
    ));

    // Only the bottom-up branch needs it: nothing is reversed otherwise, so the parity never
    // comes up.
    header.set("ROWORDER", "TOP-DOWN").unwrap();
    assert_eq!(pattern_of(&header), CfaPattern::Rggb);
}

#[test]
fn sexagesimal_hms_converts_to_ra_degrees() {
    let expected = (5.0 + 35.0 / 60.0 + 17.3 / 3600.0) * 15.0;
    for sample in ["05 35 17.3", "05:35:17.3"] {
        let degrees = parse_sexagesimal(sample).unwrap() * 15.0;
        assert!(
            (degrees - expected).abs() < 1e-10,
            "{sample}: got {degrees}, expected {expected}"
        );
    }
    assert!((parse_sexagesimal("00 00 00.0").unwrap() * 15.0).abs() < 1e-10);
}

#[test]
fn sexagesimal_dms_preserves_sign() {
    let negative = parse_sexagesimal("-05 23 28.0").unwrap();
    assert!((negative - -(5.0 + 23.0 / 60.0 + 28.0 / 3600.0)).abs() < 1e-10);
    let positive = parse_sexagesimal("+45:30:15.5").unwrap();
    assert!((positive - (45.0 + 30.0 / 60.0 + 15.5 / 3600.0)).abs() < 1e-10);
    assert!((parse_sexagesimal("-00 30 00.0").unwrap() - -0.5).abs() < 1e-10);
}

#[test]
fn invalid_sexagesimal_values_are_rejected() {
    assert!(parse_sexagesimal("05 35").is_none());
    assert!(parse_sexagesimal("").is_none());
    assert!(parse_sexagesimal("abc def ghi").is_none());
}

/// `BAYERPAT = 'TRUE'` says the frame is mosaiced and not in which phase: it is refused unless
/// the load options give the phase, which then goes through the same row-order correction a
/// stated one does.
#[test]
fn an_unstated_bayer_phase_comes_from_the_options_or_refuses_the_frame() {
    let header = bayer_header("TRUE", None, 4);
    assert!(matches!(
        read_bayer_cfa(&header, true, None),
        Err(fits_well::FitsError::TypeMismatch { name, .. }) if name == "BAYERPAT"
    ));
    assert_eq!(
        read_bayer_cfa(&header, true, Some(CfaPattern::Grbg)).unwrap(),
        Some(CfaType::Bayer(CfaPattern::Grbg))
    );
    // Bottom-up with an even height swaps GRBG's two rows: GR then BG becomes BG then GR, BGGR.
    let bottom_up = bayer_header("TRUE", Some("BOTTOM-UP"), 4);
    assert_eq!(
        read_bayer_cfa(&bottom_up, true, Some(CfaPattern::Grbg)).unwrap(),
        Some(CfaType::Bayer(CfaPattern::Bggr))
    );
    // A stated phase wins over the option, which only stands in for 'TRUE'.
    let stated = bayer_header("BGGR", None, 4);
    assert_eq!(
        read_bayer_cfa(&stated, true, Some(CfaPattern::Grbg)).unwrap(),
        Some(CfaType::Bayer(CfaPattern::Bggr))
    );
}

/// An optional keyword of the wrong type is `None`, not a failed load, and the pointing falls
/// through `RA` → `OBJCTRA` → the right-ascension WCS axis, each only when the one before gives
/// nothing usable.
#[test]
fn optional_keywords_of_the_wrong_type_are_absent() {
    let mut header = Header::new();
    header.set("RA", "05 35 17.3").unwrap();
    header.set("OBJCTRA", "05 35 17.3").unwrap();
    header.set("DEC", "not a number").unwrap();
    header.set("XBINNING", "2x2").unwrap();
    header.set("EXPTIME", "long").unwrap();
    header.set("LUMCAL", 1).unwrap();
    header.set("GAIN", 120.0).unwrap();
    header.set("CCDTEMP", -10.0).unwrap();
    let metadata = read_metadata(&header, vec![4, 4], SampleType::U16);
    assert_eq!(metadata.xbinning, None);
    assert_eq!(metadata.exposure_time, None);
    assert!(!metadata.calibrated);
    assert_eq!(metadata.gain, Some(120.0));
    assert_eq!(metadata.ccd_temp, Some(-10.0));
    assert_eq!(metadata.sample_type, Some(SampleType::U16));
    // 05h35m17.3s = 5.588139 h, times 15° per hour.
    let ra = metadata.ra_deg.unwrap();
    assert!(
        (ra - (5.0 + 35.0 / 60.0 + 17.3 / 3600.0) * 15.0).abs() < 1e-12,
        "{ra}"
    );
    assert_eq!(metadata.dec_deg, None);
}

/// The WCS reference value is taken only from an axis whose type is right ascension or
/// declination, so a swapped pair reads each from its own axis and a galactic one gives nothing.
#[test]
fn wcs_pointing_follows_the_axis_types() {
    let wcs = |types: [&str; 2]| {
        let mut header = Header::new();
        header.set("NAXIS", 2).unwrap();
        header.set("CTYPE1", types[0]).unwrap();
        header.set("CTYPE2", types[1]).unwrap();
        header.set("CRVAL1", 83.8).unwrap();
        header.set("CRVAL2", -5.4).unwrap();
        let metadata = read_metadata(&header, vec![4, 4], SampleType::F32);
        [metadata.ra_deg, metadata.dec_deg]
    };
    assert_eq!(wcs(["RA---TAN", "DEC--TAN"]), [Some(83.8), Some(-5.4)]);
    assert_eq!(wcs(["DEC--TAN", "RA---TAN"]), [Some(-5.4), Some(83.8)]);
    assert_eq!(wcs(["GLON-TAN", "GLAT-TAN"]), [None, None]);
}
