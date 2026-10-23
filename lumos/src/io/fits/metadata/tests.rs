use fits_well::header::Header;
use fits_well::image::SampleType;

use crate::image::calibration_state::CalibrationState;
use crate::image::cfa::cfa_pattern::CfaPattern;
use crate::image::cfa::cfa_type::CfaType;
use crate::image::cfa::mosaic::Mosaic;
use crate::image::dark_comparison::DarkComparison;
use crate::image::derived_facts::DerivedFacts;
use crate::image::image_metadata::ImageMetadata;
use crate::image::image_provenance::RowOrder;
use crate::image::observation_header::ObservationHeader;
use crate::image::temperature_difference::TemperatureDifference;
use crate::image::temperature_source::TemperatureSource;
use crate::internals::fits::fits_refusal;
use crate::io::fits::error::FitsRefusal;
use crate::io::fits::metadata::{
    self, Bayerpat, SOURCE_ROW_ORDER, domain_keywords, parse_sexagesimal,
    read_camera_white_balance, read_declared_row_order, read_row_order,
};

/// A minimal Bayer image header, with `ROWORDER` omitted when `roworder` is `None`.
fn bayer_header(bayerpat: &str, roworder: Option<&str>) -> Header {
    let mut header = Header::new();
    header.set("BAYERPAT", bayerpat).unwrap();
    if let Some(roworder) = roworder {
        header.set("ROWORDER", roworder).unwrap();
    }
    header
}

fn pattern_of(header: &Header, height: usize) -> CfaPattern {
    match Bayerpat::Required.read_cfa(header, height, None).unwrap() {
        Some(CfaType::Mosaic(Mosaic::Bayer(pattern))) => pattern,
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
        pattern_of(&bayer_header("RGGB", Some("BOTTOM-UP")), 4),
        CfaPattern::Gbrg
    );

    // H = 5: file row 0 is displayed row 4, which is phase 0 again, so the file reads RG then
    // GB and the declared pattern already describes it. Flipping here is what mis-debayered the
    // whole frame — and odd heights are real, LibRaw reporting 4015 for the EOS 1500D.
    assert_eq!(
        pattern_of(&bayer_header("RGGB", Some("BOTTOM-UP")), 4015),
        CfaPattern::Rggb
    );

    // Top-down, and a header that declares no order at all, leave the pattern alone whatever
    // the parity — the height only matters because reversal is what moves the phases.
    for height in [4, 5] {
        assert_eq!(
            pattern_of(&bayer_header("RGGB", Some("TOP-DOWN")), height),
            CfaPattern::Rggb,
            "top-down, height {height}"
        );
        assert_eq!(
            pattern_of(&bayer_header("RGGB", None), height),
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
            bayer_header("RGGB", Some("BOTTOM-UP"))
                .set("YBAYROFF", 1)
                .unwrap(),
            4
        ),
        CfaPattern::Rggb
    );
    assert_eq!(
        pattern_of(
            bayer_header("RGGB", Some("BOTTOM-UP"))
                .set("YBAYROFF", 1)
                .unwrap(),
            5
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
    header.set("ROWORDER", RowOrder::TopDown.keyword()).unwrap();
    header
        .set(SOURCE_ROW_ORDER, RowOrder::BottomUp.keyword())
        .unwrap();

    // The pattern is taken as written — flipping it again is what would mis-debayer the
    // reloaded frame, and is what reading `LUMROWO` here instead would have done.
    assert_eq!(read_declared_row_order(&header).unwrap(), RowOrder::TopDown);
    assert_eq!(pattern_of(&header, 4), CfaPattern::Gbrg);

    // ...while the orientation the combine compares survives the trip, so an original and a
    // copy of it written by this crate are not read as mirrored views of each other.
    assert_eq!(read_row_order(&header).unwrap(), RowOrder::BottomUp);

    // A file from anyone else has only `ROWORDER`, and then the two answers coincide.
    let third_party = bayer_header("RGGB", Some(RowOrder::BottomUp.keyword()));
    assert_eq!(
        read_declared_row_order(&third_party).unwrap(),
        RowOrder::BottomUp
    );
    assert_eq!(read_row_order(&third_party).unwrap(), RowOrder::BottomUp);
}

/// The parity comes from the decoded height, never from `NAXIS2`. A tile-compressed HDU is a binary
/// table whose `NAXIS2` counts tile rows: an fpacked frame of 4175 rows in 100-row tiles declares
/// 42, an even count, where the image's odd height leaves the pattern as declared.
#[test]
fn the_bayer_parity_follows_the_decoded_height_not_naxis2() {
    let mut header = bayer_header("RGGB", Some("BOTTOM-UP"));
    header.set("NAXIS2", 42).unwrap();
    assert_eq!(pattern_of(&header, 4175), CfaPattern::Rggb);
    assert_eq!(pattern_of(&header, 4176), CfaPattern::Gbrg);
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
    let header = bayer_header("TRUE", None);
    assert_eq!(
        fits_refusal(Bayerpat::Required.read_cfa(&header, 4, None).unwrap_err()),
        FitsRefusal::UnstatedBayerPhase
    );
    assert_eq!(
        Bayerpat::Required
            .read_cfa(&header, 4, Some(CfaPattern::Grbg))
            .unwrap(),
        Some(CfaType::Mosaic(Mosaic::Bayer(CfaPattern::Grbg)))
    );
    // Bottom-up with an even height swaps GRBG's two rows: GR then BG becomes BG then GR, BGGR.
    let bottom_up = bayer_header("TRUE", Some("BOTTOM-UP"));
    assert_eq!(
        Bayerpat::Required
            .read_cfa(&bottom_up, 4, Some(CfaPattern::Grbg))
            .unwrap(),
        Some(CfaType::Mosaic(Mosaic::Bayer(CfaPattern::Bggr)))
    );
    // A stated phase wins over the option, which only stands in for 'TRUE'.
    let stated = bayer_header("BGGR", None);
    assert_eq!(
        Bayerpat::Required
            .read_cfa(&stated, 4, Some(CfaPattern::Grbg))
            .unwrap(),
        Some(CfaType::Mosaic(Mosaic::Bayer(CfaPattern::Bggr)))
    );
}

/// An optional keyword of the wrong type, or of a value its field cannot have, is `None`, not a
/// failed load, and the field falls through to its next alias: the pointing through `RA` →
/// `OBJCTRA` → the right-ascension WCS axis, a temperature below absolute zero to the next
/// temperature keyword, a negative exposure to `EXPOSURE`, which may be 0 s as a bias's is.
#[test]
fn optional_keywords_of_the_wrong_type_are_absent() {
    let mut header = Header::new();
    header.set("RA", "05 35 17.3").unwrap();
    header.set("OBJCTRA", "05 35 17.3").unwrap();
    header.set("DEC", "not a number").unwrap();
    header.set("XBINNING", "2x2").unwrap();
    header.set("EXPTIME", "long").unwrap();
    header.set("LUMCALD", 1).unwrap();
    header.set("GAIN", 120.0).unwrap();
    header.set("CCD-TEMP", -300.0).unwrap();
    header.set("CCDTEMP", -10.0).unwrap();
    let metadata = metadata::read(&header, SampleType::U16);
    assert_eq!(metadata.header.xbinning, None);
    assert_eq!(metadata.header.exposure_time, None);
    assert_eq!(metadata.derived.calibration(), CalibrationState::NONE);
    assert_eq!(metadata.header.gain, Some(120.0));
    assert_eq!(metadata.header.ccd_temp, Some(-10.0));
    assert_eq!(metadata.header.sample_type, Some(SampleType::U16));
    // 05h35m17.3s = 5.588139 h, times 15° per hour.
    let ra = metadata.header.ra_deg.unwrap();
    assert!(
        (ra - (5.0 + 35.0 / 60.0 + 17.3 / 3600.0) * 15.0).abs() < 1e-12,
        "{ra}"
    );
    assert_eq!(metadata.header.dec_deg, None);

    header.set("EXPTIME", -1.0).unwrap();
    assert_eq!(
        metadata::read(&header, SampleType::U16)
            .header
            .exposure_time,
        None
    );
    header.set("EXPOSURE", 0.0).unwrap();
    assert_eq!(
        metadata::read(&header, SampleType::U16)
            .header
            .exposure_time,
        Some(0.0)
    );
}

/// Each numeric field holds its physical range, read through the whole header path: the value
/// at the edge it allows is kept, the one just past it is refused, and a refused first alias falls
/// through to a valid second one (`FOCALLEN = -5`, `FOCAL = 400` → 400).
#[test]
fn numeric_keywords_hold_their_ranges() {
    use crate::io::fits::metadata::{MetadataField as Field, read_data_max};

    let read = |header: &Header, field: Field| -> Option<f64> {
        let metadata = metadata::read(header, SampleType::U16);
        match field {
            Field::ExposureTime => metadata.header.exposure_time,
            Field::Gain => metadata.header.gain,
            Field::Egain => metadata.header.egain,
            Field::CcdTemp => metadata.header.ccd_temp,
            Field::CameraTemp => metadata.header.camera_temp,
            Field::SetTemp => metadata.header.set_temp,
            Field::FocalLength => metadata.header.focal_length,
            Field::Airmass => metadata.header.airmass,
            Field::PixelSizeX => metadata.header.pixel_size_x,
            Field::PixelSizeY => metadata.header.pixel_size_y,
            Field::DataMax => read_data_max(header),
            Field::Iso => metadata.header.iso.map(f64::from),
            Field::XBinning => metadata.header.xbinning.map(f64::from),
            Field::YBinning => metadata.header.ybinning.map(f64::from),
            Field::Offset => metadata.header.offset.map(f64::from),
            field => panic!("{field:?} is not numeric"),
        }
    };
    let reals = [
        (Field::ExposureTime, 0.0, -0.001),
        (Field::Gain, 0.0, -1.0),
        (Field::Egain, 0.001, 0.0),
        (Field::CcdTemp, -273.0, -273.15),
        (Field::CameraTemp, -273.0, -273.15),
        (Field::SetTemp, -273.0, -273.15),
        (Field::FocalLength, 0.5, 0.0),
        (Field::Airmass, 1.0, 0.99),
        (Field::PixelSizeX, 3.76, 0.0),
        (Field::PixelSizeY, 3.76, -3.76),
        // A saturation level of 0, a "not computed" placeholder, would flag every sample.
        (Field::DataMax, 0.5, 0.0),
    ];
    for (field, kept, refused) in reals {
        let mut header = Header::new();
        header.set(field.keywords()[0], kept).unwrap();
        assert_eq!(read(&header, field), Some(kept), "{field:?} at {kept}");
        header.set(field.keywords()[0], refused).unwrap();
        assert_eq!(read(&header, field), None, "{field:?} at {refused}");
    }
    let integers = [
        (Field::Iso, 1, Some(0)),
        (Field::XBinning, 1, Some(0)),
        (Field::YBinning, 1, Some(-2)),
        (Field::Offset, -5, None),
    ];
    for (field, kept, refused) in integers {
        let mut header = Header::new();
        header.set(field.keywords()[0], kept).unwrap();
        assert_eq!(
            read(&header, field),
            Some(f64::from(kept)),
            "{field:?} at {kept}"
        );
        if let Some(refused) = refused {
            header.set(field.keywords()[0], refused).unwrap();
            assert_eq!(read(&header, field), None, "{field:?} at {refused}");
        }
    }

    let mut header = Header::new();
    header.set("FOCALLEN", -5.0).unwrap();
    header.set("FOCAL", 400.0).unwrap();
    assert_eq!(read(&header, Field::FocalLength), Some(400.0));
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
        let metadata = metadata::read(&header, SampleType::F32);
        [metadata.header.ra_deg, metadata.header.dec_deg]
    };
    assert_eq!(wcs(["RA---TAN", "DEC--TAN"]), [Some(83.8), Some(-5.4)]);
    assert_eq!(wcs(["DEC--TAN", "RA---TAN"]), [Some(-5.4), Some(83.8)]);
    assert_eq!(wcs(["GLON-TAN", "GLAT-TAN"]), [None, None]);
}

/// Every keyword of every field reads into that field, the first spelling winning where two are
/// given, and the writer writes the first: Siril's aliases (`EXPOSURE`, `TEMPERAT`, `BINX`,
/// `PIXSIZE1`, `FRAMETYP`, `FILT-1`, `BLKLEVEL`, …) all land where the standard keyword does.
#[test]
fn every_alias_reads_into_its_field() {
    use crate::io::fits::metadata::{MetadataField, read_data_max};

    let read = |header: &Header, field: MetadataField| -> Option<String> {
        let metadata = metadata::read(header, SampleType::U16);
        let text = |value: Option<String>| value;
        let real = |value: Option<f64>| value.map(|value| format!("{value}"));
        let integer = |value: Option<i64>| value.map(|value| format!("{value}"));
        match field {
            MetadataField::Object => text(metadata.header.object),
            MetadataField::Instrument => text(metadata.header.instrument),
            MetadataField::Telescope => text(metadata.header.telescope),
            MetadataField::DateObs => text(metadata.header.date_obs),
            MetadataField::DateLocal => text(metadata.header.date_local),
            MetadataField::Filter => text(metadata.header.filter),
            MetadataField::ImageType => text(metadata.header.image_type),
            MetadataField::ExposureTime => real(metadata.header.exposure_time),
            MetadataField::Gain => real(metadata.header.gain),
            MetadataField::Egain => real(metadata.header.egain),
            MetadataField::CcdTemp => real(metadata.header.ccd_temp),
            MetadataField::CameraTemp => real(metadata.header.camera_temp),
            MetadataField::SetTemp => real(metadata.header.set_temp),
            MetadataField::FocalLength => real(metadata.header.focal_length),
            MetadataField::Airmass => real(metadata.header.airmass),
            MetadataField::PixelSizeX => real(metadata.header.pixel_size_x),
            MetadataField::PixelSizeY => real(metadata.header.pixel_size_y),
            MetadataField::DataMax => real(read_data_max(header)),
            MetadataField::Iso => integer(metadata.header.iso.map(i64::from)),
            MetadataField::XBinning => integer(metadata.header.xbinning.map(i64::from)),
            MetadataField::YBinning => integer(metadata.header.ybinning.map(i64::from)),
            MetadataField::Offset => integer(metadata.header.offset.map(i64::from)),
        }
    };
    let set = |header: &mut Header, field: MetadataField, keyword: &str, value: i32| {
        if matches!(
            field,
            MetadataField::Object
                | MetadataField::Instrument
                | MetadataField::Telescope
                | MetadataField::DateObs
                | MetadataField::DateLocal
                | MetadataField::Filter
                | MetadataField::ImageType
        ) {
            header.set(keyword, value.to_string().as_str()).unwrap();
        } else if matches!(
            field,
            MetadataField::Iso
                | MetadataField::XBinning
                | MetadataField::YBinning
                | MetadataField::Offset
        ) {
            header.set(keyword, i64::from(value)).unwrap();
        } else {
            header.set(keyword, f64::from(value)).unwrap();
        }
    };
    for field in MetadataField::ALL {
        for (value, &keyword) in (7..).zip(field.keywords()) {
            let mut header = Header::new();
            set(&mut header, field, keyword, value);
            assert_eq!(
                read(&header, field),
                Some(value.to_string()),
                "{field:?} as {keyword}"
            );
        }
        if let [first, second, ..] = field.keywords() {
            let mut header = Header::new();
            set(&mut header, field, second, 2);
            set(&mut header, field, first, 1);
            assert_eq!(
                read(&header, field),
                Some("1".to_owned()),
                "{field:?}: {first} first"
            );
        }
    }

    let mut header = Header::new();
    header.set("EXPOSURE", 30.0).unwrap();
    header.set("TEMPERAT", -10.0).unwrap();
    let metadata = metadata::read(&header, SampleType::U16);
    let mut written = Header::new();
    metadata::write_image(&mut written, &metadata, None).unwrap();
    assert_eq!(written.get_real("EXPTIME").unwrap(), Some(30.0));
    assert_eq!(written.get_real("CCD-TEMP").unwrap(), Some(-10.0));
    assert_eq!(written.get_real("EXPOSURE").unwrap(), None);
}

/// Each part calibration removed is written as its own logical keyword, and the dark comparison
/// as its reading, its exposure and any difference; each reads back as the same record, and a
/// part not set writes nothing.
#[test]
fn the_calibration_record_round_trips() {
    let unshared = DarkComparison {
        exposure_compared: true,
        temperature: None,
        temperature_difference: None,
    };
    let differing = DarkComparison {
        exposure_compared: false,
        temperature: Some(TemperatureSource::Camera),
        temperature_difference: Some(TemperatureDifference {
            reading: TemperatureSource::Camera,
            frame: 21.5,
            dark: 23.25,
        }),
    };
    for (calibration, dark, keywords) in [
        (CalibrationState::NONE, None, vec![]),
        (
            CalibrationState::BIAS,
            Some(unshared),
            vec![("LUMDKRD", "NONE")],
        ),
        (
            CalibrationState::THERMAL.union(CalibrationState::FLAT),
            Some(differing),
            vec![("LUMDKRD", "CAMERA"), ("LUMDKDRD", "CAMERA")],
        ),
        (
            CalibrationState::ADDITIVE.union(CalibrationState::FLAT),
            None,
            vec![],
        ),
    ] {
        let mut header = Header::new();
        let metadata = ImageMetadata {
            header: ObservationHeader::default(),
            derived: DerivedFacts::builder()
                .calibration(calibration)
                .dark_comparison(dark)
                .build(),
        };
        metadata::write_image(&mut header, &metadata, None).unwrap();
        for (keyword, set) in [
            ("LUMCALB", calibration.bias),
            ("LUMCALD", calibration.thermal),
            ("LUMCALF", calibration.flat),
        ] {
            assert_eq!(
                header.get_logical(keyword).unwrap(),
                set.then_some(true),
                "{calibration:?}: {keyword}"
            );
        }
        for keyword in ["LUMDKRD", "LUMDKDRD"] {
            let expected = keywords
                .iter()
                .find(|(written, _)| *written == keyword)
                .map(|(_, value)| *value);
            assert_eq!(header.get_text(keyword).unwrap(), expected, "{dark:?}");
        }
        assert_eq!(
            header.get_logical("LUMDKEXP").unwrap(),
            dark.map(|dark| dark.exposure_compared),
            "{dark:?}"
        );
        let read = metadata::read(&header, SampleType::F32);
        assert_eq!(
            (read.derived.calibration(), read.derived.dark_comparison()),
            (calibration, dark)
        );
    }
}

/// Each rule lumos holds a header to is refused as its own case: a pattern no reader knows, a
/// Bayer type without its phase, an X-Trans row missing, short, of a digit past 2, or six rows of
/// no layout, a white balance stated in part or of a multiplier at zero, and a domain keyword of a
/// value its meaning does not have.
#[test]
fn each_header_rule_is_refused_as_its_own_case() {
    let header = |entries: &[(&str, &str)]| {
        let mut header = Header::new();
        for &(keyword, value) in entries {
            header.set(keyword, value).unwrap();
        }
        header
    };
    let xtrans = |rows: [&str; 6]| {
        let mut entries = vec![("CFATYPE", "XTRANS")];
        let names = [
            "XTRNROW0", "XTRNROW1", "XTRNROW2", "XTRNROW3", "XTRNROW4", "XTRNROW5",
        ];
        entries.extend(names.into_iter().zip(rows));
        header(&entries)
    };
    let valid = ["110112", "112110", "201021", "112110", "110112", "021201"];
    assert!(metadata::read_cfa(&xtrans(valid), 6, None).is_ok());
    let mut short = valid;
    short[3] = "11211";
    let mut digit = valid;
    digit[5] = "021203";
    for (name, read, expected) in [
        (
            "an unknown CFATYPE",
            header(&[("CFATYPE", "QUAD")]),
            FitsRefusal::CfaTypeKeyword {
                value: "QUAD".to_owned(),
            },
        ),
        (
            "BAYER with no BAYERPAT",
            header(&[("CFATYPE", "BAYER")]),
            FitsRefusal::MissingBayerPattern,
        ),
        (
            "a BAYERPAT of no phase",
            header(&[("BAYERPAT", "RGBG")]),
            FitsRefusal::BayerPattern {
                value: "RGBG".to_owned(),
            },
        ),
        (
            "a missing X-Trans row",
            header(&[("CFATYPE", "XTRANS")]),
            FitsRefusal::XTransRow { row: 0 },
        ),
        (
            "a short X-Trans row",
            xtrans(short),
            FitsRefusal::XTransRow { row: 3 },
        ),
        (
            "a digit past 2",
            xtrans(digit),
            FitsRefusal::XTransRow { row: 5 },
        ),
    ] {
        assert_eq!(
            fits_refusal(metadata::read_cfa(&read, 6, None).unwrap_err()),
            expected,
            "{name}"
        );
    }
    assert!(matches!(
        fits_refusal(metadata::read_cfa(&xtrans(["111111"; 6]), 6, None).unwrap_err()),
        FitsRefusal::XTransPattern(_)
    ));

    let mut partial = Header::new();
    partial.set("LUMWBR", 2.0).unwrap();
    assert_eq!(
        fits_refusal(read_camera_white_balance(&partial).unwrap_err()),
        FitsRefusal::PartialWhiteBalance
    );
    let mut zero = partial.clone();
    for keyword in ["LUMWBG1", "LUMWBB", "LUMWBG2"] {
        zero.set(keyword, 1.0).unwrap();
    }
    assert_eq!(
        read_camera_white_balance(&zero).unwrap(),
        Some([2.0, 1.0, 1.0, 1.0])
    );
    zero.set("LUMWBB", 0.0).unwrap();
    assert_eq!(
        fits_refusal(read_camera_white_balance(&zero).unwrap_err()),
        FitsRefusal::WhiteBalanceRange
    );

    assert_eq!(
        fits_refusal(
            domain_keywords::read_origin(&header(&[("LUMSORIG", "GUESSED")])).unwrap_err()
        ),
        FitsRefusal::ScaleOrigin {
            value: "GUESSED".to_owned()
        }
    );
    assert_eq!(
        fits_refusal(domain_keywords::read_pedestal(&header(&[("LUMPED", "HALF")])).unwrap_err()),
        FitsRefusal::Pedestal {
            value: "HALF".to_owned()
        }
    );
    assert_eq!(
        fits_refusal(domain_keywords::read_pedestal(&header(&[("LUMPED", "KEPT")])).unwrap_err()),
        FitsRefusal::PedestalLevel
    );
    let mut negative = Header::new();
    negative.set("LUMQSIG", -1.0).unwrap();
    assert_eq!(
        fits_refusal(domain_keywords::read_quantization_sigma(&negative).unwrap_err()),
        FitsRefusal::KeywordOutOfRange {
            keyword: "LUMQSIG".to_owned()
        }
    );
}
