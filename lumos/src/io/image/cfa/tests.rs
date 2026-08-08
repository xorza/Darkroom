use crate::io::image::LoadContext;
use crate::io::image::cfa::*;
use crate::io::image::error::ImageError;
use crate::io::raw::demosaic::DemosaicKind;
use crate::io::raw::demosaic::xtrans::internals::test_pattern_array;
use crate::testing::make_cfa;

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
        };
        let path = common::internals::test_output_path(&format!("cfa_master_{name}.fits"));

        image.save_fits(&path).unwrap();
        let loaded = CfaImage::from_file(path, &LoadContext::default()).unwrap();

        assert_eq!(loaded.metadata.cfa_type, Some(cfa_type), "{name}");
        assert_eq!(loaded.data.pixels(), image.data.pixels(), "{name}");
    }
}

#[test]
fn test_cfa_type_mono_color_at() {
    let mono = CfaType::Mono;
    assert_eq!(mono.color_at(Vec2us::new(0, 0)), 0);
    assert_eq!(mono.color_at(Vec2us::new(5, 5)), 0);
}

#[test]
fn test_cfa_type_bayer_rggb_color_at() {
    let bayer = CfaType::Bayer(CfaPattern::Rggb);
    // RGGB: (x=0,y=0)=R, (x=1,y=0)=G, (x=0,y=1)=G, (x=1,y=1)=B
    assert_eq!(bayer.color_at(Vec2us::new(0, 0)), 0); // R
    assert_eq!(bayer.color_at(Vec2us::new(1, 0)), 1); // G
    assert_eq!(bayer.color_at(Vec2us::new(0, 1)), 1); // G
    assert_eq!(bayer.color_at(Vec2us::new(1, 1)), 2); // B
}

#[test]
fn test_cfa_type_bayer_bggr_color_at() {
    let bayer = CfaType::Bayer(CfaPattern::Bggr);
    // BGGR: (x=0,y=0)=B, (x=1,y=0)=G, (x=0,y=1)=G, (x=1,y=1)=R
    assert_eq!(bayer.color_at(Vec2us::new(0, 0)), 2); // B
    assert_eq!(bayer.color_at(Vec2us::new(1, 0)), 1); // G
    assert_eq!(bayer.color_at(Vec2us::new(0, 1)), 1); // G
    assert_eq!(bayer.color_at(Vec2us::new(1, 1)), 0); // R
}

#[test]
fn test_cfa_type_bayer_wrapping() {
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
fn test_cfa_type_xtrans_color_at() {
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
fn test_subtract() {
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
fn test_subtract_dimension_mismatch() {
    let mut light = make_cfa(Size2us::new(2, 2), vec![0.5; 4], CfaType::Mono);
    let dark = make_cfa(Size2us::new(3, 3), vec![0.1; 9], CfaType::Mono);
    light.subtract(&dark);
}

#[test]
fn test_data_len() {
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
