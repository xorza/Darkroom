use crate::frame_store::frame_stats::FrameStats;
use crate::internals::assertions::assert_close;
use crate::internals::cfa::XTRANS_PATTERN;
use crate::internals::cfa::make_cfa;
use crate::internals::fits::rewrite_fits;
use crate::internals::test_rng::TestRng;
use crate::io::image::calibration_state::CalibrationState;
use crate::io::image::cfa::*;
use crate::io::image::flat_gain::FlatGain;
use crate::io::image::sample_domain::{Pedestal, SampleDomain, ScaleOrigin};
use common::TempDir;
use fits_well::header::Header;
use std::fs;
use std::sync::Arc;

#[test]
fn a_null_is_repaired_from_its_same_colour_neighbours_before_demosaic() {
    // Mono, so the demosaic is a copy and what lands in the output is exactly what the repair put
    // there. The null holds a value with no relation to the frame — the decoder's frame-median fill
    // is what it would really be — and every one of its neighbours reads 0.5, so their median is
    // 0.5. Left alone, a mosaic demosaic would carry that 900 into every output pixel whose
    // interpolation reached it, which no mask covers.
    let size = Size2us::new(4usize, 4usize);
    let mut pixels = vec![0.5f32; size.pixel_count()];
    pixels[5] = 900.0;
    let mut nulls = vec![0.0f32; size.pixel_count()];
    nulls[5] = f32::NAN;
    let mut cfa = make_cfa(size, pixels, CfaType::Mono);
    cfa.flags = PixelFlags::of_non_finite(size, &[&nulls]);

    // A repaired null is flagged so, and a second repair leaves it as it is, though a neighbour
    // moved: the repair has one owner, and a frame calibration touched is not repaired again.
    let mut repaired = cfa.clone();
    repaired.repair_nulls();
    assert_eq!(repaired.data[5], 0.5);
    assert_eq!(
        repaired.flags.as_ref().unwrap().at(5),
        QualityFlags::NO_DATA.union(QualityFlags::REPAIRED)
    );
    repaired.data[4] = 0.25;
    repaired.repair_nulls();
    assert_eq!(repaired.data[5], 0.5);
    repaired.data[5] = 0.75;
    repaired.metadata.calibration = CalibrationState::FLAT;
    let calibrated = repaired
        .demosaic(MarkesteijnPasses::One, &CancelToken::never())
        .unwrap();
    assert_eq!(calibrated.channel(0).pixels()[5], 0.75);

    let demosaiced = cfa
        .demosaic(MarkesteijnPasses::One, &CancelToken::never())
        .unwrap();
    assert_eq!(demosaiced.channel(0).pixels()[5], 0.5);
    // The mask stays at its own extent: this pixel was reconstructed, not measured, and the combine
    // still has to gate on that.
    assert!(
        demosaiced
            .flags
            .as_ref()
            .unwrap()
            .mask_of(QualityFlags::NO_DATA)
            .get(5)
    );
    assert_eq!(
        demosaiced
            .flags
            .as_ref()
            .unwrap()
            .count(QualityFlags::NO_DATA),
        1
    );
}

/// Every flag survives the trip. `NO_DATA` goes back as NaN, the blank of the float `BITPIX`, so
/// any reader finds it; without it the repaired sample would reload as a measurement. All of them
/// go back in the `LUMFLAGS` extension, which is written only when there is more than `NO_DATA`.
///
/// The 3×2 plane holds, row-major: nothing, `NO_DATA`, `SATURATED | DEFECT` (2 + 4 = 6), nothing,
/// `COSMIC_RAY | REPAIRED` (8 + 16 = 24) and `FLAT_FLOOR` (32).
#[test]
fn a_masters_flags_survive_the_fits_round_trip() {
    let size = Size2us::new(3, 2);
    let bytes = [0u8, 1, 6, 0, 24, 32];
    let cfa = |bytes: [u8; 6]| CfaImage {
        data: Buffer2::new(3, 2, vec![0.1f32, 0.2, 0.3, 0.4, 0.5, 0.6]),
        cfa_type: CfaType::Mono,
        metadata: ImageMetadata::default(),
        flags: PixelFlags::from_fn(size, |index| QualityFlags::from_byte(bytes[index])),
    };
    let dir = TempDir::new("lumos-cfa-flags");
    let path = dir.join("master.fits");
    let hdu_count = |path: &Path| {
        let bytes = fs::read(path).unwrap();
        fits_well::FitsReader::from_bytes(&bytes)
            .unwrap()
            .hdus()
            .len()
    };

    cfa(bytes).save_fits(&path).unwrap();
    assert_eq!(hdu_count(&path), 2);
    let loaded = CfaImage::from_file(&path, &LoadContext::default()).unwrap();
    assert_eq!(loaded.flags().unwrap().bytes(), &bytes);
    assert_eq!(loaded.flags().unwrap().count(QualityFlags::NO_DATA), 1);
    // The measured samples are untouched by the trip; only the null's own value is not what was
    // written, because what was written for it was "no measurement".
    let data = loaded.data.to_vec();
    assert_eq!(
        [data[0], data[2], data[3], data[4], data[5]],
        [0.1f32, 0.3, 0.4, 0.5, 0.6]
    );

    let nulls_only = [0u8, 1, 0, 0, 0, 0];
    let nulls_path = dir.join("nulls.fits");
    cfa(nulls_only).save_fits(&nulls_path).unwrap();
    assert_eq!(hdu_count(&nulls_path), 1, "the NaN carries a lone NO_DATA");
    let loaded = CfaImage::from_file(&nulls_path, &LoadContext::default()).unwrap();
    assert_eq!(loaded.flags().unwrap().bytes(), &nulls_only);
}

/// A flat gain survives the trip in its `LUMGAIN` extension, node for node, so a light saved after
/// calibration reloads with the gain its noise model reads. A 12×6 RGGB mosaic has 4×3 nodes per
/// colour. An extension Lumos did not write is refused, each behind a valid checksum: a node that
/// is no gain, another step, another version, another grid, grids for another count of colours, and
/// one that names no image of the file.
#[test]
fn a_flat_gain_survives_the_fits_round_trip() {
    type Edit = fn(&mut Header, &mut Vec<u8>);
    let size = Size2us::new(12, 6);
    let bayer = CfaType::Bayer(CfaPattern::Rggb);
    let gain = Arc::new(FlatGain::of_divisor(
        &Buffer2::new(
            12,
            6,
            (0..size.pixel_count())
                .map(|index| 1.0 - 0.3 * (index % 12) as f32 / 12.0)
                .collect(),
        ),
        &bayer,
        |_| false,
    ));
    let dir = TempDir::new("lumos-cfa-gain");
    let path = dir.join("light.fits");
    let write = |cfa_type: CfaType| {
        CfaImage {
            data: Buffer2::new(12, 6, vec![0.25; size.pixel_count()]),
            cfa_type,
            metadata: ImageMetadata {
                flat_gain: Some(Arc::clone(&gain)),
                ..ImageMetadata::default()
            },
            flags: None,
        }
        .save_fits(&path)
        .unwrap();
    };

    write(bayer);
    let loaded = CfaImage::from_file(&path, &LoadContext::default()).unwrap();
    let reloaded = loaded.metadata.flat_gain.as_ref().unwrap();
    assert_eq!(reloaded.size(), size);
    assert!(reloaded.planes().eq(gain.planes()));
    assert!(loaded.flat_gain().is_some());

    let cases: [(&str, Edit, &str); 5] = [
        (
            "no gain",
            |_, data| data[..4].copy_from_slice(&(-1.0f32).to_be_bytes()),
            "node 0 holds -1, not a gain",
        ),
        (
            "another step",
            |header, _| {
                header.set("LUMGSTEP", 8).unwrap();
            },
            "a node every Some(8) pixels",
        ),
        (
            "another version",
            |header, _| {
                header.set("LUMOSVER", 2).unwrap();
            },
            "expected FLATGAIN version 1",
        ),
        (
            "another grid",
            |header, _| {
                header.set("NAXIS1", 3).unwrap();
                header.set("NAXIS2", 4).unwrap();
            },
            "shape [3, 4, 3] is not one or three 4x3 grids",
        ),
        (
            "no image named",
            |header, _| {
                header.set("LUMFOR", "SCI").unwrap();
            },
            "is for \"SCI\", which is no image of the file",
        ),
    ];
    for (name, edit, expected) in cases {
        write(bayer);
        rewrite_fits(&path, |index, header, data| {
            if index == 1 {
                edit(header, data);
            }
            true
        });
        let error = CfaImage::from_file(&path, &LoadContext::default()).unwrap_err();
        assert!(
            matches!(&error, ImageError::FitsUnsupported { reason, .. } if reason.contains(expected)),
            "{name}: {error:?}"
        );
    }

    // A mono frame's one plane under three colours' grids, through either loader.
    write(CfaType::Mono);
    for error in [
        CfaImage::from_file(&path, &LoadContext::default()).unwrap_err(),
        LinearImage::from_file(&path, &LoadContext::default()).unwrap_err(),
    ] {
        assert!(
            matches!(&error, ImageError::FitsUnsupported { reason, .. }
                if reason.contains("holds 3 grids for an image of 1 colours or channels")),
            "{error:?}"
        );
    }
}

/// A flags extension that is not the one Lumos wrote is refused, each behind a valid checksum so
/// the check behind it is what fires: a bit no flag has, a `NO_DATA` the NaNs do not state either
/// way, another geometry, another version, and an extension that names no image of the file. Its
/// checksum is judged before its bytes: a byte changed behind the stored checksum reads as that,
/// not as the `NO_DATA` it also drops.
#[test]
fn a_flags_extension_lumos_did_not_write_is_refused() {
    type Edit = fn(&mut Header, &mut Vec<u8>);
    let size = Size2us::new(3, 2);
    let bytes = [0u8, 1, 6, 0, 24, 32];
    let dir = TempDir::new("lumos-cfa-flags-refused");
    let path = dir.join("master.fits");
    let write = || {
        CfaImage {
            data: Buffer2::new(3, 2, vec![0.1f32, 0.2, 0.3, 0.4, 0.5, 0.6]),
            cfa_type: CfaType::Mono,
            metadata: ImageMetadata::default(),
            flags: PixelFlags::from_fn(size, |index| QualityFlags::from_byte(bytes[index])),
        }
        .save_fits(&path)
        .unwrap();
    };
    let cases: [(&str, Edit, &str); 6] = [
        (
            "unknown bit",
            |_, data| data[5] |= 0x40,
            "(2, 1): byte 0x60 holds a bit no flag has",
        ),
        (
            "NO_DATA lost",
            |_, data| data[1] = 0,
            "(1, 0): NO_DATA disagrees",
        ),
        (
            "NO_DATA added",
            |_, data| data[3] = 1,
            "(0, 1): NO_DATA disagrees",
        ),
        (
            "another geometry",
            |header, _| {
                header.set("NAXIS1", 2).unwrap();
                header.set("NAXIS2", 3).unwrap();
            },
            "shape [2, 3] is not the image's 3x2",
        ),
        (
            "another version",
            |header, _| {
                header.set("LUMOSVER", 2).unwrap();
            },
            "expected PIXFLAGS version 1",
        ),
        (
            "no image named",
            |header, _| {
                header.set("LUMFOR", "SCI").unwrap();
            },
            "is for \"SCI\", which is no image of the file",
        ),
    ];
    for (name, edit, expected) in cases {
        write();
        rewrite_fits(&path, |index, header, data| {
            if index == 1 {
                edit(header, data);
            }
            true
        });
        let error = CfaImage::from_file(&path, &LoadContext::default()).unwrap_err();
        assert!(
            matches!(&error, ImageError::FitsUnsupported { reason, .. } if reason.contains(expected)),
            "{name}: {error:?}"
        );
    }

    // The flags are the last HDU, and their 6 bytes open its one 2880-byte data block.
    write();
    let mut file = fs::read(&path).unwrap();
    let flags_start = file.len() - 2880;
    file[flags_start + 1] = 0;
    fs::write(&path, &file).unwrap();
    let error = CfaImage::from_file(&path, &LoadContext::default()).unwrap_err();
    assert!(
        matches!(&error, ImageError::FitsUnsupported { reason, .. }
            if reason.contains("HDU 1 requires valid DATASUM and CHECKSUM")),
        "{error:?}"
    );
}

/// The demosaic measures each colour's noise on the mosaic, before the interpolation correlates
/// neighbours, and the frame's statistics take it from there: the demosaiced frame's noise, sky
/// and quantization σ are the mosaic's own, bit for bit, though the demosaic cleared the frame's
/// quantization σ. Measured on the frame's own correlated pixels instead, every channel reads less
/// than its colour's σ. A 64 × 64 RGGB mosaic: red 0.125 ± 0.02, green 0.25 ± 0.01, blue
/// 0.375 ± 0.03, with a step of 1/4096. Divided by a flat, the frame keeps the flat's gain through
/// the demosaic, and its noise's split by that gain is the mosaic's too.
#[test]
fn a_demosaiced_frame_keeps_its_mosaics_noise() {
    let size = Size2us::new(64, 64);
    let cfa_type = CfaType::Bayer(CfaPattern::Rggb);
    let (level, sigma) = ([0.125f32, 0.25, 0.375], [0.02f32, 0.01, 0.03]);
    let mut rng = TestRng::new(7);
    let pixels = (0..size.pixel_count())
        .map(|index| {
            let colour = usize::from(cfa_type.color_at(Vec2us::new(index % 64, index / 64)));
            level[colour] + sigma[colour] * rng.next_gaussian_f32()
        })
        .collect();
    let mut cfa = make_cfa(size, pixels, cfa_type);
    cfa.metadata.quantization_sigma = Some(QUANTIZATION_SIGMA_PER_STEP / 4096.0);
    let gain = Arc::new(FlatGain::of_divisor(
        &Buffer2::new(
            64,
            64,
            (0..size.pixel_count())
                .map(|index| 1.0 - 0.5 * (index % 64) as f32 / 64.0)
                .collect(),
        ),
        &cfa_type,
        |_| false,
    ));
    let mut flattened = cfa.clone();
    flattened.metadata.flat_gain = Some(Arc::clone(&gain));
    let flattened_mosaic = FrameStats::measure(&flattened);
    let flattened = flattened
        .demosaic(MarkesteijnPasses::One, &CancelToken::never())
        .unwrap();
    assert!(Arc::ptr_eq(
        flattened.metadata.flat_gain.as_ref().unwrap(),
        &gain
    ));
    let flattened_frame = FrameStats::measure(&flattened);
    assert_eq!(flattened_frame.noise, flattened_mosaic.noise);
    assert_eq!(flattened_frame.read_share, flattened_mosaic.read_share);

    let mosaic = FrameStats::measure(&cfa);
    let mut demosaiced = cfa
        .demosaic(MarkesteijnPasses::One, &CancelToken::never())
        .unwrap();
    assert_eq!(demosaiced.metadata.quantization_sigma, None);
    let frame = FrameStats::measure(&demosaiced);
    assert_eq!(frame.noise, mosaic.noise);
    assert_eq!(frame.sky, mosaic.sky);
    assert_eq!(frame.quantization_sigma, mosaic.quantization_sigma);
    assert_eq!(frame.read_share.as_slice(), [0.0; 3]);

    demosaiced.metadata.mosaic_noise = None;
    let correlated = FrameStats::measure(&demosaiced);
    for colour in 0..3 {
        assert!(
            correlated.noise[colour] < mosaic.noise[colour],
            "colour {colour}: {} against the mosaic's {}",
            correlated.noise[colour],
            mosaic.noise[colour]
        );
    }
}

/// A master records its whole domain and its quantization σ, so a reload gives back exactly what
/// was saved, for every origin and every pedestal: the σ is not divided by the span a second time
/// (15360× too small for a RAW master), and an assumed scale reloads as assumed rather than as a
/// scale of 1 the master would be refused for.
#[test]
fn a_masters_sample_domain_and_quantization_survive_the_fits_round_trip() {
    let directory = TempDir::new("lumos-cfa-domain");
    let mut case = 0;
    for origin in [ScaleOrigin::Declared, ScaleOrigin::Assumed] {
        for pedestal in [Pedestal::Removed, Pedestal::Kept(2048.0), Pedestal::Unknown] {
            for (scale, unit) in [
                (15_360.0, None),
                (1_234.567_8, Some("ADU")),
                (65_535.0, None),
            ] {
                case += 1;
                let mut master = make_cfa(Size2us::new(2, 2), vec![0.25; 4], CfaType::Mono);
                let domain = SampleDomain {
                    scale,
                    origin,
                    pedestal,
                    unit: unit.map(str::to_owned),
                };
                master.metadata.domain = Some(domain.clone());
                master.metadata.quantization_sigma = Some(QUANTIZATION_SIGMA_PER_STEP / 15_360.0);
                let path = directory.path().join(format!("master_{case}.fits"));
                master.save_fits(&path).unwrap();
                let loaded = CfaImage::from_file(&path, &LoadContext::default()).unwrap();

                assert_eq!(loaded.metadata.domain, Some(domain.clone()), "{domain}");
                assert_eq!(
                    loaded.metadata.quantization_sigma, master.metadata.quantization_sigma,
                    "{domain}"
                );
                assert_eq!(
                    loaded.data.to_vec(),
                    vec![0.25; 4],
                    "samples are stored normalized"
                );
                assert_eq!(
                    loaded.metadata.domain.unwrap().conversion_to(&domain),
                    Some(DomainMap::IDENTITY),
                    "{domain}"
                );
            }
        }
    }
}

#[test]
fn master_cfa_save_load_round_trips_data_and_pattern() {
    let cfa = CfaImage {
        data: Buffer2::new(2, 2, vec![0.1f32, 0.2, 0.3, 0.4]),
        cfa_type: CfaType::Bayer(CfaPattern::Bggr),
        metadata: ImageMetadata {
            camera_white_balance: Some([2.0, 1.0, 1.5, 1.0]),
            quantization_sigma: Some(0.000_01),
            ..Default::default()
        },
        flags: None,
    };
    let dir = TempDir::new("lumos-cfa-roundtrip");
    let path = dir.join("master.fits");
    cfa.save_fits(&path).unwrap();
    let info = CfaFrameInfo::from_file(&path, &LoadContext::default()).unwrap();
    assert_eq!(info.dimensions, ImageDimensions::new((2, 2), 1));
    assert_eq!(info.cfa_type, CfaType::Bayer(CfaPattern::Bggr));
    let loaded = CfaImage::from_file(&path, &LoadContext::default()).unwrap();

    assert_eq!((loaded.data.width(), loaded.data.height()), (2, 2));
    assert_eq!(loaded.data.to_vec(), vec![0.1f32, 0.2, 0.3, 0.4]);
    assert_eq!(loaded.cfa_type, CfaType::Bayer(CfaPattern::Bggr));
    assert_eq!(
        loaded.metadata.camera_white_balance,
        Some([2.0, 1.0, 1.5, 1.0])
    );
    assert_eq!(loaded.metadata.quantization_sigma, Some(0.000_01));

    let original = fs::read(&path).unwrap();
    let mut invalid_version = original.clone();
    let version_card = invalid_version
        .windows(8)
        .position(|window| window == b"LUMOSVER")
        .unwrap();
    let version_digit = invalid_version[version_card..version_card + 80]
        .iter()
        .rposition(u8::is_ascii_digit)
        .unwrap();
    invalid_version[version_card + version_digit] = b'0';
    fs::write(&path, invalid_version).unwrap();
    assert!(matches!(
        CfaImage::from_file(&path, &LoadContext::default()),
        Err(ImageError::FitsUnsupported { reason, .. }) if reason.contains("version")
    ));

    let mut corrupted = original;
    let sample = 0.1f32.to_be_bytes();
    let offset = corrupted
        .windows(sample.len())
        .position(|window| window == sample)
        .unwrap();
    corrupted[offset] ^= 0x01;
    fs::write(&path, corrupted).unwrap();
    let error = CfaImage::from_file(&path, &LoadContext::default()).unwrap_err();
    assert!(
        matches!(
            &error,
            ImageError::FitsUnsupported { reason, .. }
                if reason.contains("requires valid DATASUM and CHECKSUM")
        ),
        "{error:?}"
    );
}

#[test]
fn master_cfa_fits_round_trips_mono_and_xtrans_patterns() {
    let dir = TempDir::new("lumos-cfa-types");
    for (name, cfa_type) in [
        ("mono", CfaType::Mono),
        ("xtrans", CfaType::XTrans(XTRANS_PATTERN)),
    ] {
        let image = CfaImage {
            data: Buffer2::new(2, 2, vec![0.1f32, 0.2, 0.3, 0.4]),
            cfa_type,
            metadata: ImageMetadata::default(),
            flags: None,
        };
        let path = dir.join(format!("master_{name}.fits"));

        image.save_fits(&path).unwrap();
        let loaded = CfaImage::from_file(path, &LoadContext::default()).unwrap();

        assert_eq!(loaded.cfa_type, cfa_type, "{name}");
        assert_eq!(loaded.data.pixels(), image.data.pixels(), "{name}");
    }
}

#[test]
fn subtract_takes_the_dark_off_every_sample() {
    let mut light = make_cfa(Size2us::new(2, 2), vec![0.5, 0.6, 0.7, 0.8], CfaType::Mono);
    let dark = make_cfa(Size2us::new(2, 2), vec![0.1, 0.1, 0.1, 0.1], CfaType::Mono);

    light.subtract(&dark, DomainMap::IDENTITY);

    assert_close!(light.data[0], 0.4, 1e-6);
    assert_close!(light.data[1], 0.5, 1e-6);
    assert_close!(light.data[2], 0.6, 1e-6);
    assert_close!(light.data[3], 0.7, 1e-6);
}

/// The dark is expressed in the light's domain before it is subtracted: on a span four times the
/// light's, a dark sample of 0.125 is worth 0.5, and an offset of −0.25 moves its pedestal onto the
/// light's. Dyadic values, so the result is exact.
#[test]
fn subtract_converts_the_dark_into_the_lights_domain_first() {
    let mut light = make_cfa(
        Size2us::new(2, 2),
        vec![0.75, 1.0, 0.5, 0.625],
        CfaType::Mono,
    );
    let dark = make_cfa(Size2us::new(2, 2), vec![0.125; 4], CfaType::Mono);

    light.subtract(
        &dark,
        DomainMap {
            gain: 4.0,
            offset: 0.0,
        },
    );
    assert_eq!(light.data.pixels(), &[0.25, 0.5, 0.0, 0.125]);

    let mut light = make_cfa(
        Size2us::new(2, 2),
        vec![0.75, 1.0, 0.5, 0.625],
        CfaType::Mono,
    );
    light.subtract(
        &dark,
        DomainMap {
            gain: 4.0,
            offset: -0.25,
        },
    );
    assert_eq!(light.data.pixels(), &[0.5, 0.75, 0.25, 0.375]);
}

#[test]
#[should_panic(expected = "dimensions mismatch")]
fn subtract_dimension_mismatch() {
    let mut light = make_cfa(Size2us::new(2, 2), vec![0.5; 4], CfaType::Mono);
    let dark = make_cfa(Size2us::new(3, 3), vec![0.1; 9], CfaType::Mono);
    light.subtract(&dark, DomainMap::IDENTITY);
}

#[test]
fn data_len() {
    let img = CfaImage::from_plane(
        Buffer2::new(10, 20, vec![0.0; 200]),
        CfaType::Mono,
        ImageMetadata::default(),
    );
    assert_eq!(img.data.len(), 200);
    assert_eq!(img.metadata.quantization_sigma, None);
}

/// LibRaw's `filters`, `colors` and `cdesc` classify a sensor: one colour is mono whatever the word
/// says, `filters == 0` with three colours is a linear DNG or sRAW that LibRaw processes itself,
/// `9` is X-Trans with the pattern LibRaw keeps apart, a 2-row-periodic word is Bayer, and any
/// other word is an exotic CFA that LibRaw processes too. Four colours are no RGB mosaic whatever
/// the word: the Sony DSC-F828's `0x9c9c9c9c` reads R, E / G, B (emerald where a Bayer phase has
/// green) and the Nikon E950's CMYG `0x1e1e1e1e` reads as BGGR's phases; LibRaw describes them
/// `RGBE` and `CMYG`, with four colours.
#[test]
fn from_libraw_classifies_the_sensor() {
    let xtrans = *XTRANS_PATTERN.rows();
    let classify =
        |filters, colors| CfaType::from_libraw(filters, colors, *b"RGBG", xtrans).unwrap();
    assert_eq!(classify(0, 1), Some(CfaType::Mono));
    assert_eq!(classify(0x9494_9494, 1), Some(CfaType::Mono));
    assert_eq!(classify(0, 3), None);
    assert_eq!(classify(9, 3), Some(CfaType::XTrans(XTRANS_PATTERN)));
    assert_eq!(
        classify(0x9494_9494, 3),
        Some(CfaType::Bayer(CfaPattern::Rggb))
    );
    assert_eq!(
        classify(0x1616_1616, 3),
        Some(CfaType::Bayer(CfaPattern::Bggr))
    );
    assert_eq!(classify(0x1234_5678, 3), None);
    for (filters, cdesc) in [(0x9c9c_9c9c, *b"RGBE"), (0x1e1e_1e1e, *b"CMYG")] {
        assert!(
            CfaPattern::from_filters(filters).is_some(),
            "{cdesc:?}: the word alone passes for Bayer"
        );
        assert_eq!(
            CfaType::from_libraw(filters, 4, cdesc, xtrans).unwrap(),
            None,
            "{cdesc:?}"
        );
        assert_eq!(
            CfaType::from_libraw(filters, 3, cdesc, xtrans).unwrap(),
            None,
            "{cdesc:?} described as three colours"
        );
    }
    // An X-Trans sensor whose layout is not one is a corrupt file, refused with the reason.
    let mut corrupt = xtrans;
    corrupt[0][0] = 3;
    assert!(matches!(
        CfaType::from_libraw(9, 3, *b"RGBG", corrupt),
        Err(XTransPatternError::Value {
            row: 0,
            column: 0,
            value: 3
        })
    ));
}

/// Each pattern names the demosaic lumos runs on it and what its output colour means.
#[test]
fn each_pattern_names_its_demosaic() {
    for (cfa_type, demosaic, color) in [
        (
            CfaType::Mono,
            DemosaicProvenance::None,
            ColorProvenance::Monochrome,
        ),
        (
            CfaType::Bayer(CfaPattern::Rggb),
            DemosaicProvenance::LumosRcd,
            ColorProvenance::SensorRgb,
        ),
        (
            CfaType::XTrans(XTRANS_PATTERN),
            DemosaicProvenance::LumosMarkesteijn {
                passes: MarkesteijnPasses::Three,
            },
            ColorProvenance::SensorRgb,
        ),
    ] {
        assert_eq!(
            cfa_type.demosaic_provenance(MarkesteijnPasses::Three),
            demosaic,
            "{cfa_type:?}"
        );
        assert_eq!(cfa_type.demosaiced_color(), color, "{cfa_type:?}");
    }
}

/// The colour at every position of two periods, for each pattern, from each source that states it:
/// the `BAYERPAT` spelling read letter by letter, LibRaw's `filters` word, `CfaPattern`, and
/// `CfaType`; mono is red-index 0 everywhere and X-Trans is its rows, both wrapping.
#[test]
fn every_pattern_names_the_colour_at_each_position() {
    let colour_of = |letter: u8| match letter {
        b'R' => 0,
        b'G' => 1,
        b'B' => 2,
        _ => unreachable!(),
    };
    for (pattern, filters) in [
        (CfaPattern::Rggb, 0x9494_9494),
        (CfaPattern::Bggr, 0x1616_1616),
        (CfaPattern::Grbg, 0x6161_6161),
        (CfaPattern::Gbrg, 0x4949_4949),
    ] {
        let spelling = pattern.bayerpat().as_bytes();
        for y in 0..4 {
            for x in 0..4 {
                let expected = colour_of(spelling[(y % 2) * 2 + x % 2]);
                let pos = Vec2us::new(x, y);
                assert_eq!(pattern.color_at(pos) as u8, expected, "{pattern:?} {pos:?}");
                assert_eq!(CfaType::Bayer(pattern).color_at(pos), expected);
                assert_eq!(raw::libraw_filter_color(filters, y, x) as u8, expected);
            }
        }
    }
    for y in 0..12 {
        for x in 0..12 {
            let pos = Vec2us::new(x, y);
            assert_eq!(CfaType::Mono.color_at(pos), 0);
            assert_eq!(
                CfaType::XTrans(XTRANS_PATTERN).color_at(pos),
                XTRANS_PATTERN.rows()[y % 6][x % 6]
            );
        }
    }
}

/// No input photosite moves an output pixel farther away than `CfaType::demosaic_support`. An
/// impulse of 20 on random texture, at several phases of each pattern, changes no output pixel past
/// the support by more than 2⁻¹⁶ of the impulse. A wider survey, 40 textures at 6 phases on 128²
/// frames, reached exactly 10 for RCD and 11 and 16 for Markesteijn's one and three passes, by any
/// change at all for Markesteijn; this sample reaches 9, 11 and 14.
#[test]
fn the_demosaic_support_bounds_every_impulse_response() {
    const SIDE: usize = 64;
    for (cfa_type, passes) in [
        (CfaType::Bayer(CfaPattern::Rggb), MarkesteijnPasses::One),
        (CfaType::XTrans(XTRANS_PATTERN), MarkesteijnPasses::One),
        (CfaType::XTrans(XTRANS_PATTERN), MarkesteijnPasses::Three),
    ] {
        let size = Size2us::new(SIDE, SIDE);
        let mut reach = 0usize;
        for seed in 1..=6u64 {
            let mut rng = TestRng::new(seed);
            let base: Vec<f32> = (0..size.pixel_count())
                .map(|_| 0.2 + 0.3 * rng.next_f32())
                .collect();
            let reference = make_cfa(size, base.clone(), cfa_type)
                .demosaic(passes, &CancelToken::never())
                .unwrap();
            for (cx, cy) in [(30usize, 30usize), (31, 30), (31, 31), (33, 32)] {
                let mut pixels = base.clone();
                pixels[cy * SIDE + cx] = 20.0;
                let threshold = (20.0 - base[cy * SIDE + cx]) / 65_536.0;
                let hit = make_cfa(size, pixels, cfa_type)
                    .demosaic(passes, &CancelToken::never())
                    .unwrap();
                for channel in 0..3 {
                    for y in 0..SIDE {
                        for x in 0..SIDE {
                            let moved = (hit.channel(channel)[(x, y)]
                                - reference.channel(channel)[(x, y)])
                                .abs();
                            if moved > threshold {
                                reach = reach.max(x.abs_diff(cx).max(y.abs_diff(cy)));
                            }
                        }
                    }
                }
            }
        }
        assert!(
            reach <= cfa_type.demosaic_support(passes),
            "{cfa_type:?}, {passes:?}: {reach}"
        );
    }
}

/// A flag on one photosite covers every output pixel the demosaic reads it into, and `NO_DATA`
/// stays where it was: a saturated photosite at (20, 20) flags the 21 × 21 square around it.
#[test]
fn demosaic_spreads_flags_by_its_support() {
    let size = Size2us::new(48usize, 48usize);
    let mut cfa = make_cfa(
        size,
        vec![0.25; size.pixel_count()],
        CfaType::Bayer(CfaPattern::Rggb),
    );
    cfa.flags = PixelFlags::from_fn(size, |index| match index {
        index if index == 20 * 48 + 20 => QualityFlags::SATURATED,
        index if index == 40 * 48 + 40 => QualityFlags::NO_DATA,
        _ => QualityFlags::default(),
    });
    let flags = cfa
        .demosaic(MarkesteijnPasses::One, &CancelToken::never())
        .unwrap()
        .flags
        .unwrap();
    assert_eq!(flags.count(QualityFlags::SATURATED), 21 * 21);
    assert_eq!(flags.count(QualityFlags::NO_DATA), 1);
    assert!(
        flags
            .at_pos(Vec2us::new(10, 30))
            .intersects(QualityFlags::SATURATED)
    );
    assert!(
        !flags
            .at_pos(Vec2us::new(31, 20))
            .intersects(QualityFlags::SATURATED)
    );
}

/// Neutral detail through a sensor whose channels respond with gains 1/2, 1 and 1/1.5 keeps less
/// false colour when the camera balance is applied before the demosaic: the direction decisions
/// compare neighbours of different colours, and on the unbalanced mosaic the cast reads as
/// structure. Balanced back up, red and blue should equal green; their RMS difference over the
/// interior, measured, is 0.0013 against 0.0156 for RCD on a star of σ 1.3 px and 0.0008 against
/// 0.0032 on a soft edge, and 0.0045 against 0.023 and 0.0008 against 0.0094 for Markesteijn. The
/// test asks for a third of the unbalanced error at most, below the least ratio measured, 4.2.
#[test]
fn a_balanced_demosaic_keeps_neutral_detail_neutral() {
    let size = Size2us::new(48, 48);
    let gains = [2.0f32, 1.0, 1.5];
    let star = |x: usize, y: usize| {
        let (dx, dy) = (x as f32 - 23.6, y as f32 - 24.3);
        0.1 + 0.8 * (-(dx * dx + dy * dy) / (2.0 * 1.3 * 1.3)).exp()
    };
    let soft_edge =
        |x: usize, y: usize| 0.5 + 0.3 * ((x as f32 + 0.37 * y as f32 - 30.0) / 1.5).tanh();
    let scenes: [&dyn Fn(usize, usize) -> f32; 2] = [&star, &soft_edge];
    for cfa_type in [
        CfaType::Bayer(CfaPattern::Rggb),
        CfaType::XTrans(XTRANS_PATTERN),
    ] {
        for (scene_index, scene) in scenes.iter().enumerate() {
            let samples: Vec<f32> = (0..size.pixel_count())
                .map(|index| {
                    let (x, y) = (index % size.width, index / size.width);
                    scene(x, y) / gains[cfa_type.color_at(Vec2us::new(x, y)) as usize]
                })
                .collect();
            let false_colour = |balance: Option<[f32; 4]>| {
                let mut cfa = make_cfa(size, samples.clone(), cfa_type);
                cfa.metadata.camera_white_balance = balance;
                let image = cfa
                    .demosaic(MarkesteijnPasses::One, &CancelToken::never())
                    .unwrap();
                let (mut sum, mut count) = (0.0f64, 0);
                for y in 12..36 {
                    for x in 12..36 {
                        let green = image.channel(1)[(x, y)];
                        for (channel, gain) in [(0, gains[0]), (2, gains[2])] {
                            let error = image.channel(channel)[(x, y)] * gain - green;
                            sum += f64::from(error * error);
                            count += 1;
                        }
                    }
                }
                (sum / f64::from(count)).sqrt()
            };
            let balanced = false_colour(Some([2.0, 1.0, 1.5, 1.0]));
            let unbalanced = false_colour(None);
            assert!(
                balanced * 3.0 < unbalanced,
                "{cfa_type:?} scene {scene_index}: balanced {balanced}, unbalanced {unbalanced}"
            );
        }
    }
}

/// The green proxy keeps every green photosite and fills the others from the greens of their 3×3
/// neighbourhood, weighed 1 beside and 1/√2 on a diagonal, over the mosaic `v = x + 10y`.
///
/// RGGB: red (0, 0) has greens (1, 0) and (0, 1) beside it, 1 and 10, so 5.5; blue (1, 1) has
/// greens 10, 12, 1 and 21 beside it, so 11 — a Bayer site's diagonals are never green. X-Trans:
/// red (2, 0) has greens (1, 0) and (3, 0) beside it, 1 and 3, and (1, 1) and (3, 1) on its
/// diagonals, 11 and 13, so `(4 + 24r) / (2 + 2r)` with `r = 1/√2`. A filled photosite carries
/// the flags of the greens it was filled from, a green its own: green (1, 0) is saturated, and
/// so are the red (0, 0) and blue (1, 1) it fills, while blue (3, 3), whose greens are (2, 3),
/// (4, 3), (3, 2) and (3, 4), carries nothing.
#[test]
fn the_green_proxy_fills_each_photosite_from_its_greens() {
    let size = Size2us::new(6, 6);
    let pixels: Vec<f32> = (0..size.pixel_count())
        .map(|index| (index % 6 + 10 * (index / 6)) as f32)
        .collect();
    let mut bayer = make_cfa(size, pixels.clone(), CfaType::Bayer(CfaPattern::Rggb));
    PixelFlags::add_where(&mut bayer.flags, size, QualityFlags::SATURATED, |index| {
        index == 1
    });
    let proxy = bayer.green_proxy();
    let plane = proxy.channel(0);
    assert_eq!(plane[(0, 0)], 5.5);
    assert_eq!(plane[(1, 1)], 11.0);
    assert_eq!(plane[(1, 0)], 1.0, "a green photosite is kept");
    let flags = proxy.flags.as_ref().unwrap();
    for (pixel, expected) in [
        (1, QualityFlags::SATURATED),
        (0, QualityFlags::SATURATED),
        (7, QualityFlags::SATURATED),
        (21, QualityFlags::default()),
    ] {
        assert_eq!(flags.at(pixel), expected, "pixel {pixel}");
    }
    assert_eq!(
        proxy.metadata.provenance.as_ref().map(|p| p.demosaic),
        bayer
            .metadata
            .provenance
            .as_ref()
            .map(|_| DemosaicProvenance::GreenProxy)
    );

    let xtrans = make_cfa(size, pixels, CfaType::XTrans(XTRANS_PATTERN)).green_proxy();
    let r = FRAC_1_SQRT_2;
    let expected = ((4.0 + 24.0 * r) / (2.0 + 2.0 * r)) as f32;
    let actual = xtrans.channel(0)[(2, 0)];
    assert!(
        (actual - expected).abs() <= f32::EPSILON * expected,
        "{actual}, expected {expected}"
    );
}
