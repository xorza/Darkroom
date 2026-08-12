use crate::io::image::cfa::*;
use crate::io::image::error::ImageError;
use crate::io::image::load_context::LoadContext;
use crate::io::raw::demosaic::DemosaicKind;
use crate::io::raw::demosaic::xtrans::internals::test_pattern_array;
use crate::testing::make_cfa;

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
    cfa.nulls = NullMask::of_non_finite(size, &[&nulls]);

    let demosaiced = cfa.demosaic(&CancelToken::never()).unwrap();
    assert_eq!(demosaiced.channel(0).pixels()[5], 0.5);
    // The mask stays at its own extent: this pixel was reconstructed, not measured, and the combine
    // still has to gate on that.
    assert!(demosaiced.nulls.as_ref().unwrap().is_null(5));
    assert_eq!(demosaiced.nulls.as_ref().unwrap().count(), 1);
}

#[test]
fn a_masters_nulls_survive_the_fits_round_trip() {
    // Written back as NaN — the blank for the float BITPIX this writes — so a reload recovers the
    // mask. Without it the repaired sample would come back as a measurement, which is exactly the
    // fabrication the mask exists to prevent.
    let cfa = CfaImage {
        data: Buffer2::new(2, 2, vec![0.1f32, 0.2, 0.3, 0.4]),
        metadata: ImageMetadata {
            cfa_type: Some(CfaType::Mono),
            ..Default::default()
        },
        quantization_sigma: None,
        nulls: NullMask::of_non_finite(Size2us::new(2usize, 2usize), &[&[0.0, f32::NAN, 0.0, 0.0]]),
    };
    let path = common::internals::test_output_path("cfa_master_nulls_roundtrip.fits");
    cfa.save_fits(&path).unwrap();

    let loaded = CfaImage::from_file(&path, &LoadContext::default()).unwrap();
    let nulls = loaded.nulls.as_ref().expect("the mask must come back");
    assert_eq!(nulls.count(), 1);
    for index in 0..4 {
        assert_eq!(nulls.is_null(index), index == 1, "index {index}");
    }
    // The measured samples are untouched by the trip; only the null's own value is not what was
    // written, because what was written for it was "no measurement".
    let data = loaded.data.to_vec();
    assert_eq!([data[0], data[2], data[3]], [0.1f32, 0.3, 0.4]);
}

#[test]
fn master_cfa_save_load_round_trips_data_and_pattern() {
    let cfa = CfaImage {
        data: Buffer2::new(2, 2, vec![0.1f32, 0.2, 0.3, 0.4]),
        metadata: ImageMetadata {
            cfa_type: Some(CfaType::Bayer(CfaPattern::Bggr)),
            camera_white_balance: Some([2.0, 1.0, 1.5, 1.0]),
            ..Default::default()
        },
        quantization_sigma: Some(0.000_01),
        nulls: None,
    };
    let path = common::internals::test_output_path("cfa_master_roundtrip.fits");
    cfa.save_fits(&path).unwrap();
    let info = CfaFrameInfo::from_file(&path, &LoadContext::default()).unwrap();
    assert_eq!(info.dimensions, ImageDimensions::new((2, 2), 1));
    assert_eq!(info.demosaic, DemosaicKind::BayerRcd);
    let loaded = CfaImage::from_file(&path, &LoadContext::default()).unwrap();

    assert_eq!((loaded.data.width(), loaded.data.height()), (2, 2));
    assert_eq!(loaded.data.to_vec(), vec![0.1f32, 0.2, 0.3, 0.4]);
    assert!(matches!(
        loaded.metadata.cfa_type,
        Some(CfaType::Bayer(CfaPattern::Bggr))
    ));
    assert_eq!(
        loaded.metadata.camera_white_balance,
        Some([2.0, 1.0, 1.5, 1.0])
    );
    assert_eq!(loaded.quantization_sigma, Some(0.000_01));

    let original = std::fs::read(&path).unwrap();
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
    std::fs::write(&path, invalid_version).unwrap();
    assert!(matches!(
        CfaImage::from_file(&path, &LoadContext::default()),
        Err(ImageError::FitsUnsupported { reason, .. }) if reason.contains("version")
    ));

    let mut corrupted = original.clone();
    let sample = 0.1f32.to_be_bytes();
    let offset = corrupted
        .windows(sample.len())
        .position(|window| window == sample)
        .unwrap();
    corrupted[offset] ^= 0x01;
    std::fs::write(&path, corrupted).unwrap();
    let error = CfaImage::from_file(&path, &LoadContext::default()).unwrap_err();
    assert!(
        matches!(
            &error,
            ImageError::FitsUnsupported { reason, .. }
                if reason.contains("requires valid DATASUM and CHECKSUM")
        ),
        "{error:?}"
    );
    std::fs::write(path, original).unwrap();
}

#[test]
fn master_cfa_fits_round_trips_mono_and_xtrans_patterns() {
    for (name, cfa_type) in [
        ("mono", CfaType::Mono),
        ("xtrans", CfaType::XTrans(test_pattern_array())),
    ] {
        let image = CfaImage {
            data: Buffer2::new(2, 2, vec![0.1f32, 0.2, 0.3, 0.4]),
            metadata: ImageMetadata {
                cfa_type: Some(cfa_type.clone()),
                ..Default::default()
            },
            quantization_sigma: None,
            nulls: None,
        };
        let path = common::internals::test_output_path(&format!("cfa_master_{name}.fits"));

        image.save_fits(&path).unwrap();
        let loaded = CfaImage::from_file(path, &LoadContext::default()).unwrap();

        assert_eq!(loaded.metadata.cfa_type, Some(cfa_type), "{name}");
        assert_eq!(loaded.data.pixels(), image.data.pixels(), "{name}");
    }
}

#[test]
fn cfa_type_mono_color_at() {
    let mono = CfaType::Mono;
    assert_eq!(mono.color_at(Vec2us::new(0, 0)), 0);
    assert_eq!(mono.color_at(Vec2us::new(5, 5)), 0);
}

#[test]
fn cfa_type_bayer_rggb_color_at() {
    let bayer = CfaType::Bayer(CfaPattern::Rggb);
    // RGGB: (x=0,y=0)=R, (x=1,y=0)=G, (x=0,y=1)=G, (x=1,y=1)=B
    assert_eq!(bayer.color_at(Vec2us::new(0, 0)), 0); // R
    assert_eq!(bayer.color_at(Vec2us::new(1, 0)), 1); // G
    assert_eq!(bayer.color_at(Vec2us::new(0, 1)), 1); // G
    assert_eq!(bayer.color_at(Vec2us::new(1, 1)), 2); // B
}

#[test]
fn cfa_type_bayer_bggr_color_at() {
    let bayer = CfaType::Bayer(CfaPattern::Bggr);
    // BGGR: (x=0,y=0)=B, (x=1,y=0)=G, (x=0,y=1)=G, (x=1,y=1)=R
    assert_eq!(bayer.color_at(Vec2us::new(0, 0)), 2); // B
    assert_eq!(bayer.color_at(Vec2us::new(1, 0)), 1); // G
    assert_eq!(bayer.color_at(Vec2us::new(0, 1)), 1); // G
    assert_eq!(bayer.color_at(Vec2us::new(1, 1)), 0); // R
}

#[test]
fn cfa_type_bayer_wrapping() {
    let bayer = CfaType::Bayer(CfaPattern::Rggb);
    // Pattern repeats every 2 pixels
    assert_eq!(
        bayer.color_at(Vec2us::new(0, 0)),
        bayer.color_at(Vec2us::new(2, 0))
    );
    assert_eq!(
        bayer.color_at(Vec2us::new(0, 0)),
        bayer.color_at(Vec2us::new(0, 2))
    );
    assert_eq!(
        bayer.color_at(Vec2us::new(1, 1)),
        bayer.color_at(Vec2us::new(3, 3))
    );
}

#[test]
fn cfa_type_xtrans_color_at() {
    let pattern = [
        [1, 0, 1, 1, 2, 1],
        [2, 1, 2, 0, 1, 0],
        [1, 2, 1, 1, 0, 1],
        [1, 2, 1, 1, 0, 1],
        [0, 1, 0, 2, 1, 2],
        [1, 0, 1, 1, 2, 1],
    ];
    let xtrans = CfaType::XTrans(pattern);
    assert_eq!(xtrans.color_at(Vec2us::new(0, 0)), 1); // G
    assert_eq!(xtrans.color_at(Vec2us::new(1, 0)), 0); // R
    assert_eq!(xtrans.color_at(Vec2us::new(0, 1)), 2); // B
    // Wrapping
    assert_eq!(
        xtrans.color_at(Vec2us::new(6, 0)),
        xtrans.color_at(Vec2us::new(0, 0))
    );
    assert_eq!(
        xtrans.color_at(Vec2us::new(0, 6)),
        xtrans.color_at(Vec2us::new(0, 0))
    );
}

#[test]
fn subtract_takes_the_dark_off_every_sample() {
    let mut light = make_cfa(Size2us::new(2, 2), vec![0.5, 0.6, 0.7, 0.8], CfaType::Mono);
    let dark = make_cfa(Size2us::new(2, 2), vec![0.1, 0.1, 0.1, 0.1], CfaType::Mono);

    light.subtract(&dark);

    assert!((light.data[0] - 0.4).abs() < 1e-6);
    assert!((light.data[1] - 0.5).abs() < 1e-6);
    assert!((light.data[2] - 0.6).abs() < 1e-6);
    assert!((light.data[3] - 0.7).abs() < 1e-6);
}

#[test]
#[should_panic(expected = "dimensions mismatch")]
fn subtract_dimension_mismatch() {
    let mut light = make_cfa(Size2us::new(2, 2), vec![0.5; 4], CfaType::Mono);
    let dark = make_cfa(Size2us::new(3, 3), vec![0.1; 9], CfaType::Mono);
    light.subtract(&dark);
}

#[test]
fn data_len() {
    let img = CfaImage::from_plane(
        Buffer2::new(10, 20, vec![0.0; 200]),
        ImageMetadata {
            cfa_type: Some(CfaType::Mono),
            ..ImageMetadata::default()
        },
    );
    assert_eq!(img.data.len(), 200);
    assert_eq!(img.quantization_sigma, None);
}
