//! Load/decode round-trip tests on synthetic frames.
//!
//! `fits-well` ships a `FitsWriter`, so a synthetic FITS can be written and read back through the
//! real `load_linear_fits` path — exercising BitPix selection, the unsigned-via-BZERO convention,
//! the division of integer samples into the `[0, 1]` domain (and the float path's exemption from
//! it), and null rejection. The demosaic path is exercised by building mosaics from known colours
//! and demosaicing them back.

use crate::testing::prelude::*;
use std::fs::File;

use crate::io::image::error::ImageError;
use crate::io::image::fits::decode::{load_cfa_fits, load_linear_fits};
use crate::io::image::load_context::LoadContext;
use crate::io::raw::demosaic::bayer::CfaPattern;
use crate::io::raw::demosaic::xtrans::internals::test_pattern_array;
use crate::stacking::frame_store::StackableImage;
use crate::testing::make_cfa;
use crate::{CalibrationMasters, CalibrationSet, CfaImage, CfaType, PreviewImage};
use fits_well::header::Header;
use fits_well::image::{Image, Scaling};
use fits_well::{FitsError, FitsWriter};
use imaginarium::ColorFormat;

/// Write `image` to a temp FITS file via `FitsWriter`, then load it through `load_linear_fits`.
fn write_and_load(name: &str, image: &Image) -> Result<LinearImage, ImageError> {
    let path = common::internals::test_output_path(&format!("fits_roundtrip/{name}.fits"));
    let mut writer = FitsWriter::new(File::create(&path).unwrap());
    writer.write_image(image).unwrap();
    writer.into_inner().sync_all().unwrap();
    load_linear_fits(&path, &LoadContext::default())
}

fn write_with_header(name: &str, image: &Image, header: &Header) -> std::path::PathBuf {
    let path = common::internals::test_output_path(&format!("fits_roundtrip/{name}.fits"));
    let mut writer = FitsWriter::new(File::create(&path).unwrap());
    writer.write_image_with_header(image, header).unwrap();
    writer.into_inner().sync_all().unwrap();
    path
}

fn write_header_and_load(name: &str, header: &Header) -> Result<LinearImage, ImageError> {
    let path = common::internals::test_output_path(&format!("fits_roundtrip/{name}.fits"));
    let mut writer = FitsWriter::new(File::create(&path).unwrap());
    writer.write_raw_hdu(header, &0.0f32.to_be_bytes()).unwrap();
    writer.into_inner().sync_all().unwrap();
    load_linear_fits(&path, &LoadContext::default())
}

fn one_pixel_header() -> Header {
    let mut header = Header::new();
    header.set("SIMPLE", true).unwrap();
    header.set("BITPIX", -32).unwrap();
    header.set("NAXIS", 2).unwrap();
    header.set("NAXIS1", 1).unwrap();
    header.set("NAXIS2", 1).unwrap();
    header
}

#[test]
fn fits_metadata_errors_survive_the_lumos_loader() {
    let mut mistyped = one_pixel_header();
    mistyped.set("DATAMAX", "not a real").unwrap();
    assert!(matches!(
        write_header_and_load("mistyped_metadata", &mistyped),
        Err(ImageError::Fits {
            source: FitsError::TypeMismatch { name, expected },
            ..
        }) if name == "DATAMAX" && expected == "real"
    ));

    let mut out_of_range = one_pixel_header();
    out_of_range.set("ISOSPEED", -1).unwrap();
    assert!(matches!(
        write_header_and_load("out_of_range_metadata", &out_of_range),
        Err(ImageError::Fits {
            source: FitsError::KeywordOutOfRange { name: "ISOSPEED" },
            ..
        })
    ));
}

#[test]
fn fits_float32_round_trips_pixels_and_order() {
    let size = Size2us::new(32usize, 24usize);
    // The asymmetric physical scale catches both transposition and accidental frame-max scaling.
    let pixels: Vec<f32> = (0..size.height)
        .flat_map(|y| (0..size.width).map(move |x| -12.0 + y as f32 * 3.0 + x as f32 * 0.25))
        .collect();
    let image = Image::new(
        vec![size.width, size.height], // fits-well is NAXIS1-first: [width, height]
        pixels.clone(),
    )
    .unwrap();

    let loaded = write_and_load("float32", &image).unwrap();
    assert_eq!(loaded.width(), size.width);
    assert_eq!(loaded.height(), size.height);
    assert_eq!(loaded.channels(), 1);
    assert_eq!(loaded.channel(0).pixels(), pixels);
}

#[test]
fn fits_integer_samples_are_divided_by_the_span_their_header_declares() {
    // Every case divides by |BSCALE| × (2^bits − 1) — the span the header itself declares — so
    // frames from different integer widths and BSCALEs land in one comparable domain. The scale
    // is applied without an offset, so a signed frame keeps its own zero.

    // BITPIX = 16, BSCALE = 1: divisor 65535, physical -32768..=32767 → about [-0.5, 0.5].
    let signed = Image::new(vec![4, 1], vec![-32_768i16, -3, 0, 32_767]).unwrap();
    let signed_loaded = write_and_load("int16", &signed).unwrap();
    let pixels = signed_loaded.channel(0).pixels();
    assert!((pixels[0] - -0.500_007_6).abs() < 1e-7, "{pixels:?}");
    assert!((pixels[1] - -4.577_636_7e-5).abs() < 1e-9, "{pixels:?}");
    assert_eq!(pixels[2], 0.0);
    assert!((pixels[3] - 0.499_992_37).abs() < 1e-7, "{pixels:?}");

    // BSCALE = -2.5 widens the declared span to 2.5 × 65535 = 163837.5, and the physical values
    // 17.5, 10, 0 divide by it. A negative BSCALE scales by its magnitude: the sign already lives
    // in the physical value fits-well produced.
    let scaled = Image::new_scaled(
        vec![3, 1],
        vec![-3i16, 0, 4],
        Scaling {
            bscale: -2.5,
            bzero: 10.0,
            blank: None,
        },
    )
    .unwrap();
    let scaled_loaded = write_and_load("negative_bscale", &scaled).unwrap();
    let pixels = scaled_loaded.channel(0).pixels();
    // The 2.5 cancels: 17.5 / (2.5 × 65535) = 7/65535 = 1.0681315e-4, and
    // 10 / (2.5 × 65535) = 4/65535 = 6.1036087e-5.
    assert!((pixels[0] - 1.068_131_5e-4).abs() < 1e-9, "{pixels:?}");
    assert!((pixels[1] - 6.103_609e-5).abs() < 1e-9, "{pixels:?}");
    assert_eq!(pixels[2], 0.0);

    // The headline case: the FITS unsigned convention (BZERO = 2¹⁵) lands exactly on [0, 1].
    let size = Size2us::new(5usize, 1usize);
    let raw = [0u16, 16384, 32768, 49152, 65535];
    let image = Image::from_u16(vec![size.width, size.height], &raw).unwrap();

    let loaded = write_and_load("uint16", &image).unwrap();
    let pixels = loaded.channel(0).pixels();
    assert_eq!(pixels[0], 0.0);
    assert_eq!(pixels[4], 1.0);
    for (index, &value) in raw.iter().enumerate() {
        let expected = f32::from(value) / 65_535.0;
        assert!(
            (pixels[index] - expected).abs() < 1e-7,
            "sample {index}: {pixels:?}"
        );
    }
}

#[test]
fn fits_quantization_sigma_follows_the_samples_into_the_normalized_domain() {
    // σ describes one ADC step, so it is only comparable to the samples if it is divided by the
    // same span they were. Both the BSCALE-derived step and a file's declared QNTZSIG go through
    // that division, which is what keeps two frames' σ values commensurate.
    let raw = [0u16, 16384, 32768, 65535];
    let image = Image::from_u16(vec![4, 1], &raw).unwrap();

    let mut header = Header::new();
    header.set("BAYERPAT", "RGGB").unwrap();
    let path = write_with_header("cfa_uint16_sigma", &image, &header);
    let loaded = load_cfa_fits(&path, &LoadContext::default()).unwrap();
    // BITPIX = 16, BSCALE = 1 → divisor 65535, so one ADU is 1/65535 and its uniform-error σ is
    // (1/√12) / 65535 = 0.28867513 / 65535 = 4.4049001e-6.
    let sigma = loaded.quantization_sigma.unwrap();
    assert!((sigma - 4.404_9e-6).abs() < 1e-11, "{sigma}");

    // A declared QNTZSIG is in the file's sample units and takes the same division: 2 ADU maps to
    // 2 / 65535 = 3.05181e-5, not to 2.
    let mut declared = Header::new();
    declared.set("BAYERPAT", "RGGB").unwrap();
    declared.set("QNTZSIG", 2.0).unwrap();
    let declared_path = write_with_header("cfa_uint16_declared_sigma", &image, &declared);
    let declared_loaded = load_cfa_fits(&declared_path, &LoadContext::default()).unwrap();
    let declared_sigma = declared_loaded.quantization_sigma.unwrap();
    assert!(
        (declared_sigma - 2.0 / 65_535.0).abs() < 1e-11,
        "{declared_sigma}"
    );
    assert!(
        (declared_sigma - 3.051_81e-5).abs() < 1e-9,
        "{declared_sigma}"
    );
}

#[test]
fn fits_float_samples_are_normalized_only_when_datamax_declares_them_adu() {
    // A float BITPIX declares no full scale, so DATAMAX is the only evidence. The decision is
    // taken from the header alone — never from the pixels, which would give each frame its own
    // divisor and break the commensurability the division exists to establish. These three files
    // hold *identical* samples and differ only in their header.
    let pixels = vec![-5.0f32, 0.0, 0.5, 65_535.0];
    let image = Image::new(vec![4, 1], pixels.clone()).unwrap();

    // No DATAMAX: taken as already normalized. This is the Lumos-written master's case, and it is
    // also the one third-party ADU file this rule cannot rescue.
    let bare = write_and_load("float32_no_datamax", &image).unwrap();
    assert_eq!(bare.channel(0).pixels(), &pixels[..]);

    // DATAMAX ≈ 1: a normalized frame saying so. Left alone.
    let mut normalized_header = Header::new();
    normalized_header.set("DATAMAX", 1.0).unwrap();
    let normalized_path = write_with_header("float32_datamax_1", &image, &normalized_header);
    let normalized = load_linear_fits(&normalized_path, &LoadContext::default()).unwrap();
    assert_eq!(normalized.channel(0).pixels(), &pixels[..]);
    assert_eq!(normalized.metadata.data_max, Some(1.0));

    // DATAMAX = 65535: a saturation level far above unity, so the samples are ADU and divide by
    // the 16-bit full scale. -5/65535 = -7.629511e-5, 0.5/65535 = 7.629511e-6, 65535/65535 = 1.
    let mut adu_header = Header::new();
    adu_header.set("DATAMAX", 65_535.0).unwrap();
    let adu_path = write_with_header("float32_datamax_adu", &image, &adu_header);
    let adu = load_linear_fits(&adu_path, &LoadContext::default()).unwrap();
    let decoded = adu.channel(0).pixels();
    assert!((decoded[0] - -7.629_511e-5).abs() < 1e-9, "{decoded:?}");
    assert_eq!(decoded[1], 0.0);
    assert!((decoded[2] - 7.629_511e-6).abs() < 1e-10, "{decoded:?}");
    assert_eq!(decoded[3], 1.0);
    // DATAMAX follows the samples, so the round-trip is stable: saving and reloading this frame
    // sees DATAMAX = 1 and leaves it alone rather than dividing a second time.
    assert_eq!(adu.metadata.data_max, Some(1.0));
}

#[test]
fn fits_datamax_follows_the_samples_into_the_normalized_domain() {
    // DATAMAX is a saturation level in the file's sample units, so it is divided by the same span
    // the samples were: it stays a threshold the decoded samples can be compared against, and it
    // still does not influence what those samples decode to.
    let image = Image::new(vec![4, 1], vec![-7i16, 0, 41, 300]).unwrap();
    let mut low_header = Header::new();
    low_header.set("DATAMAX", 100.0).unwrap();
    let mut high_header = Header::new();
    high_header.set("DATAMAX", 65_535.0).unwrap();
    let low_path = write_with_header("datamax_100", &image, &low_header);
    let high_path = write_with_header("datamax_65535", &image, &high_header);

    let low = load_linear_fits(&low_path, &LoadContext::default()).unwrap();
    let high = load_linear_fits(&high_path, &LoadContext::default()).unwrap();

    // BITPIX = 16, BSCALE = 1 → divisor 65535, and the samples are identical either way.
    let pixels = low.channel(0).pixels();
    assert_eq!(high.channel(0).pixels(), pixels);
    assert_eq!(pixels[1], 0.0);
    for (index, physical) in [-7.0f32, 0.0, 41.0, 300.0].into_iter().enumerate() {
        let expected = physical / 65_535.0;
        assert!(
            (pixels[index] - expected).abs() < 1e-9,
            "sample {index}: {pixels:?}"
        );
    }

    // 100 / 65535 = 1.5259e-3; 65535 / 65535 = 1 exactly.
    let low_max = low.metadata.data_max.unwrap();
    assert!((low_max - 1.525_902_2e-3).abs() < 1e-9, "{low_max}");
    assert_eq!(high.metadata.data_max, Some(1.0));
}

#[test]
fn mosaic_fits_uses_the_cfa_calibration_route() {
    let size = Size2us::new(32usize, 32usize);
    let pattern = CfaType::Bayer(CfaPattern::Rggb);
    let target = [0.8f32, 0.5, 0.2];
    let dark_value = 0.1f32;
    let pixels: Vec<f32> = (0..size.height)
        .flat_map(|y| {
            let pattern = pattern.clone();
            (0..size.width)
                .map(move |x| target[pattern.color_at(Vec2us::new(x, y)) as usize] + dark_value)
        })
        .collect();
    let image = Image::new(vec![size.width, size.height], pixels.clone()).unwrap();
    let mut header = Header::new();
    header.set("BAYERPAT", "RGGB").unwrap();
    let path = write_with_header("bayer_cfa", &image, &header);

    assert!(matches!(
        LinearImage::from_file(&path, &LoadContext::default()),
        Err(ImageError::ScientificInputRejected { .. })
    ));
    let mut loaded = CfaImage::from_file(&path, &LoadContext::default()).unwrap();
    assert_eq!(loaded.data.pixels(), pixels);
    assert_eq!(loaded.metadata.cfa_type, Some(pattern.clone()));
    let cache_loaded = <CfaImage as StackableImage>::load(&path, &LoadContext::default()).unwrap();
    assert_eq!(cache_loaded.data, loaded.data);
    assert_eq!(cache_loaded.metadata.cfa_type, loaded.metadata.cfa_type);
    assert_eq!(
        <CfaImage as StackableImage>::peek_dimensions(&path, &LoadContext::default()),
        Some(crate::ImageDimensions::new((size.width, size.height), 1))
    );

    let preview = PreviewImage::from_file(&path, &LoadContext::default()).unwrap();
    assert!(matches!(
        &preview.metadata.provenance,
        Some(crate::ImageProvenance {
            color: crate::ColorProvenance::SensorRgb,
            demosaic: crate::DemosaicProvenance::LumosRcd,
            ..
        })
    ));
    let preview: imaginarium::Image = preview.into();
    assert_eq!(preview.desc().color_format, ColorFormat::RGB_F32);
    let preview_pixels = bytemuck::cast_slice::<u8, f32>(preview.bytes());
    for y in 6..size.height - 6 {
        for x in 6..size.width - 6 {
            let channel = pattern.color_at(Vec2us::new(x, y)) as usize;
            assert_eq!(
                preview_pixels[(size.index_of(Vec2us::new(x, y))) * 3 + channel],
                target[channel] + dark_value
            );
        }
    }

    let dark = make_cfa(size, vec![dark_value; size.pixel_count()], pattern.clone());
    let masters = CalibrationMasters::from_images(
        CalibrationSet {
            dark: Some(dark),
            flat: None,
            bias: None,
            flat_dark: None,
        },
        5.0,
        CancelToken::never(),
    )
    .unwrap();
    let mut equivalent = make_cfa(size, pixels, pattern.clone());
    masters.calibrate(&mut loaded).unwrap();
    masters.calibrate(&mut equivalent).unwrap();
    assert_eq!(loaded.data, equivalent.data);

    let demosaiced = loaded.demosaic(&CancelToken::never()).unwrap();
    let equivalent_demosaiced = equivalent.demosaic(&CancelToken::never()).unwrap();
    for channel in 0..3 {
        assert_eq!(
            demosaiced.channel(channel),
            equivalent_demosaiced.channel(channel)
        );
    }
    assert!(matches!(
        demosaiced.metadata.provenance,
        Some(crate::ImageProvenance {
            container: crate::SourceContainer::Fits,
            transfer: crate::TransferProvenance::FitsNormalized(crate::FitsTransferProvenance {
                bscale: 1.0,
                bzero: 0.0,
                // A float FITS carries no declared full scale, so it is taken as already
                // normalized and its samples pass through undivided.
                physical_scale: 1.0,
                ..
            },),
            color: crate::ColorProvenance::SensorRgb,
            demosaic: crate::DemosaicProvenance::LumosRcd,
            clipped: false,
            ..
        })
    ));
    for y in 6..size.height - 6 {
        for x in 6..size.width - 6 {
            let channel = pattern.color_at(Vec2us::new(x, y)) as usize;
            let expected = (target[channel] + dark_value) - dark_value;
            assert_eq!(
                demosaiced.channel(channel)[size.index_of(Vec2us::new(x, y))],
                expected
            );
        }
    }
}

#[test]
fn fits_rejects_nan_and_inf_with_summary() {
    let size = Size2us::new(4usize, 4usize);
    let mut pixels = vec![0.3f32; size.pixel_count()];
    pixels[0] = f32::NAN;
    pixels[5] = f32::INFINITY;
    pixels[10] = f32::NEG_INFINITY;
    let image = Image::new(vec![size.width, size.height], pixels).unwrap();

    assert!(matches!(
        write_and_load("nan_inf", &image),
        Err(ImageError::FitsUnsupported { reason, .. })
            if reason == "image contains 3 null/non-finite pixels in a decode chunk; first at linear index 0"
    ));
}

#[test]
fn demosaic_uniform_bayer_recovers_colour() {
    let size = Size2us::new(32usize, 32usize);
    let rgb = [0.8f32, 0.5, 0.2]; // R, G, B
    let cfa = CfaType::Bayer(CfaPattern::Rggb);

    // Sample each Bayer site from the (uniform) true colour.
    let mut mosaic = vec![0.0f32; size.pixel_count()];
    for y in 0..size.height {
        for x in 0..size.width {
            mosaic[size.index_of(Vec2us::new(x, y))] =
                rgb[cfa.color_at(Vec2us::new(x, y)) as usize];
        }
    }
    let image = make_cfa(size, mosaic, cfa)
        .demosaic(&CancelToken::never())
        .unwrap();

    // A uniform colour must demosaic back to that colour. RCD is gradient-based, so a perfectly
    // flat field is a degenerate (zero-gradient) input with a few ratio artifacts — but recovery
    // must be *unbiased*: the interior mean of every channel matches the true colour, and the
    // typical pixel is close (median deviation small).
    let channels = [
        image.channel(0).pixels(),
        image.channel(1).pixels(),
        image.channel(2).pixels(),
    ];
    for (ch, &true_c) in channels.iter().zip(&rgb) {
        let mut devs: Vec<f32> = Vec::new();
        let mut sum = 0.0f64;
        for y in 6..size.height - 6 {
            for x in 6..size.width - 6 {
                let v = ch[size.index_of(Vec2us::new(x, y))];
                sum += v as f64;
                devs.push((v - true_c).abs());
            }
        }
        let mean = (sum / devs.len() as f64) as f32;
        devs.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let median_dev = devs[devs.len() / 2];
        assert!(
            (mean - true_c).abs() < 0.01,
            "interior mean {mean} should recover channel colour {true_c}"
        );
        assert!(
            median_dev < 0.01,
            "the typical interior pixel should match {true_c}, median deviation {median_dev}"
        );
    }
}

#[test]
fn calibrated_demosaic_preserves_out_of_range_samples() {
    let size = Size2us::new(48usize, 48usize);

    for cfa in [
        CfaType::Bayer(CfaPattern::Rggb),
        CfaType::XTrans(test_pattern_array()),
    ] {
        for expected in [-0.25f32, 1.25] {
            let image = make_cfa(size, vec![expected; size.pixel_count()], cfa.clone())
                .demosaic(&CancelToken::never())
                .unwrap();

            for channel in 0..3 {
                let pixels = image.channel(channel).pixels();
                assert!(pixels.iter().all(|sample| sample.is_finite()));
                for y in 8..size.height - 8 {
                    for x in 8..size.width - 8 {
                        let actual = pixels[size.index_of(Vec2us::new(x, y))];
                        assert!(
                            (actual - expected).abs() < 1e-4,
                            "{cfa:?} channel {channel} at ({x},{y}) changed uniform {expected} to {actual}"
                        );
                    }
                }
            }
        }
    }
}
