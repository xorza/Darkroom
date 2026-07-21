//! Load/decode round-trip tests on synthetic frames.
//!
//! `fits-well` ships a `FitsWriter`, so a synthetic FITS can be written and read back through the
//! real `load_linear_fits` path — exercising BitPix selection, the unsigned-via-BZERO convention, the
//! physical integer/float preservation and null rejection. The demosaic path is
//! exercised by building mosaics from known colours and demosaicing them back.

use std::fs::File;

use crate::io::image::error::ImageError;
use crate::io::image::fits::load_linear_fits;
use crate::io::image::linear::LinearImage;
use crate::io::image::LoadContext;
use crate::io::raw::demosaic::bayer::CfaPattern;
use crate::io::raw::demosaic::xtrans::test_support::test_pattern_array;
use crate::stacking::frame_store::StackableImage;
use crate::testing::make_cfa;
use crate::{CalibrationMasters, CalibrationSet, CfaImage, CfaType, PreviewImage};
use common::CancelToken;
use fits_well::header::Header;
use fits_well::image::{Image, Scaling};
use fits_well::{FitsError, FitsWriter};
use imaginarium::ColorFormat;

/// Write `image` to a temp FITS file via `FitsWriter`, then load it through `load_linear_fits`.
fn write_and_load(name: &str, image: &Image) -> Result<LinearImage, ImageError> {
    let path = common::test_utils::test_output_path(&format!("fits_roundtrip/{name}.fits"));
    let mut writer = FitsWriter::new(File::create(&path).unwrap());
    writer.write_image(image).unwrap();
    writer.into_inner().sync_all().unwrap();
    load_linear_fits(&path, &LoadContext::default())
}

fn write_with_header(name: &str, image: &Image, header: &Header) -> std::path::PathBuf {
    let path = common::test_utils::test_output_path(&format!("fits_roundtrip/{name}.fits"));
    let mut writer = FitsWriter::new(File::create(&path).unwrap());
    writer.write_image_with_header(image, header).unwrap();
    writer.into_inner().sync_all().unwrap();
    path
}

fn write_header_and_load(name: &str, header: &Header) -> Result<LinearImage, ImageError> {
    let path = common::test_utils::test_output_path(&format!("fits_roundtrip/{name}.fits"));
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
    let (w, h) = (32usize, 24usize);
    // The asymmetric physical scale catches both transposition and accidental frame-max scaling.
    let pixels: Vec<f32> = (0..h)
        .flat_map(|y| (0..w).map(move |x| -12.0 + y as f32 * 3.0 + x as f32 * 0.25))
        .collect();
    let image = Image::new(
        vec![w, h], // fits-well is NAXIS1-first: [width, height]
        pixels.clone(),
    )
    .unwrap();

    let loaded = write_and_load("float32", &image).unwrap();
    assert_eq!(loaded.width(), w);
    assert_eq!(loaded.height(), h);
    assert_eq!(loaded.channels(), 1);
    assert_eq!(loaded.channel(0).pixels(), pixels);
}

#[test]
fn fits_signed_scaled_and_unsigned_samples_remain_physical() {
    let signed = Image::new(vec![4, 1], vec![-32_768i16, -3, 0, 32_767]).unwrap();
    let signed_loaded = write_and_load("int16", &signed).unwrap();
    assert_eq!(
        signed_loaded.channel(0).pixels(),
        &[-32_768.0, -3.0, 0.0, 32_767.0]
    );

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
    assert_eq!(scaled_loaded.channel(0).pixels(), &[17.5, 10.0, 0.0]);

    let (w, h) = (5usize, 1usize);
    let raw = [0u16, 16384, 32768, 49152, 65535];
    let image = Image::from_u16(vec![w, h], &raw).unwrap();

    let loaded = write_and_load("uint16", &image).unwrap();
    let expected: Vec<f32> = raw.iter().map(|&value| value as f32).collect();
    assert_eq!(loaded.channel(0).pixels(), expected);
}

#[test]
fn fits_datamax_is_metadata_only() {
    let image = Image::new(vec![4, 1], vec![-7i16, 0, 41, 300]).unwrap();
    let mut low_header = Header::new();
    low_header.set("DATAMAX", 100.0).unwrap();
    let mut high_header = Header::new();
    high_header.set("DATAMAX", 65_535.0).unwrap();
    let low_path = write_with_header("datamax_100", &image, &low_header);
    let high_path = write_with_header("datamax_65535", &image, &high_header);

    let low = load_linear_fits(&low_path, &LoadContext::default()).unwrap();
    let high = load_linear_fits(&high_path, &LoadContext::default()).unwrap();
    assert_eq!(low.channel(0).pixels(), &[-7.0, 0.0, 41.0, 300.0]);
    assert_eq!(high.channel(0).pixels(), low.channel(0).pixels());
    assert_eq!(low.metadata.data_max, Some(100.0));
    assert_eq!(high.metadata.data_max, Some(65_535.0));
}

#[test]
fn mosaic_fits_uses_the_cfa_calibration_route() {
    let (width, height) = (32usize, 32usize);
    let pattern = CfaType::Bayer(CfaPattern::Rggb);
    let target = [0.8f32, 0.5, 0.2];
    let dark_value = 0.1f32;
    let pixels: Vec<f32> = (0..height)
        .flat_map(|y| {
            let pattern = pattern.clone();
            (0..width).map(move |x| target[pattern.color_at(x, y) as usize] + dark_value)
        })
        .collect();
    let image = Image::new(vec![width, height], pixels.clone()).unwrap();
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
    let cache_loaded =
        <CfaImage as StackableImage>::load(&path, &LoadContext::default()).unwrap();
    assert_eq!(cache_loaded.data, loaded.data);
    assert_eq!(cache_loaded.metadata.cfa_type, loaded.metadata.cfa_type);
    assert_eq!(
        <CfaImage as StackableImage>::peek_dimensions(&path, &LoadContext::default()),
        Some(crate::ImageDimensions::new((width, height), 1))
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
    for y in 6..height - 6 {
        for x in 6..width - 6 {
            let channel = pattern.color_at(x, y) as usize;
            assert_eq!(
                preview_pixels[(y * width + x) * 3 + channel],
                target[channel] + dark_value
            );
        }
    }

    let dark = make_cfa(
        width,
        height,
        vec![dark_value; width * height],
        pattern.clone(),
    );
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
    let mut equivalent = make_cfa(width, height, pixels, pattern.clone());
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
            transfer: crate::TransferProvenance::FitsPhysical {
                bscale: 1.0,
                bzero: 0.0,
                ..
            },
            color: crate::ColorProvenance::SensorRgb,
            demosaic: crate::DemosaicProvenance::LumosRcd,
            clipped: false,
            ..
        })
    ));
    for y in 6..height - 6 {
        for x in 6..width - 6 {
            let channel = pattern.color_at(x, y) as usize;
            let expected = (target[channel] + dark_value) - dark_value;
            assert_eq!(demosaiced.channel(channel)[y * width + x], expected);
        }
    }
}

#[test]
fn fits_rejects_nan_and_inf_with_summary() {
    let (w, h) = (4usize, 4usize);
    let mut pixels = vec![0.3f32; w * h];
    pixels[0] = f32::NAN;
    pixels[5] = f32::INFINITY;
    pixels[10] = f32::NEG_INFINITY;
    let image = Image::new(vec![w, h], pixels).unwrap();

    assert!(matches!(
        write_and_load("nan_inf", &image),
        Err(ImageError::FitsUnsupported { reason, .. })
            if reason == "image contains 3 null/non-finite pixels; first at linear index 0"
    ));
}

#[test]
fn demosaic_uniform_bayer_recovers_colour() {
    let (w, h) = (32usize, 32usize);
    let rgb = [0.8f32, 0.5, 0.2]; // R, G, B
    let cfa = CfaType::Bayer(CfaPattern::Rggb);

    // Sample each Bayer site from the (uniform) true colour.
    let mut mosaic = vec![0.0f32; w * h];
    for y in 0..h {
        for x in 0..w {
            mosaic[y * w + x] = rgb[cfa.color_at(x, y) as usize];
        }
    }
    let image = make_cfa(w, h, mosaic, cfa)
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
        for y in 6..h - 6 {
            for x in 6..w - 6 {
                let v = ch[y * w + x];
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
    let (w, h) = (48usize, 48usize);

    for cfa in [
        CfaType::Bayer(CfaPattern::Rggb),
        CfaType::XTrans(test_pattern_array()),
    ] {
        for expected in [-0.25f32, 1.25] {
            let image = make_cfa(w, h, vec![expected; w * h], cfa.clone())
                .demosaic(&CancelToken::never())
                .unwrap();

            for channel in 0..3 {
                let pixels = image.channel(channel).pixels();
                assert!(pixels.iter().all(|sample| sample.is_finite()));
                for y in 8..h - 8 {
                    for x in 8..w - 8 {
                        let actual = pixels[y * w + x];
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
