//! Characterization snapshots: bit-exact digests of each pipeline stage's output on fixed
//! synthetic input, so a rewrite that moves a result names the stage it moved.
//!
//! A snapshot pins what the code does, not what is right. A change that moves one states why;
//! one that moves without a stated reason is a regression until shown otherwise. The digests are
//! those of an `x86_64` host with AVX2 and FMA: the vector backends and fused multiply-adds round
//! differently on any other host, which reports the snapshots as skipped rather than failing them.

mod snapshot;

use crate::image_ops::stretching::Stretch;
use crate::io::image::cfa::{CfaImage, CfaType};
use crate::io::image::fits::cfa::save_cfa_fits;
use crate::io::image::load_context::LoadContext;
use crate::io::raw::demosaic::bayer::CfaPattern;
use crate::stacking::calibration_masters::{
    CalibrationMasters, CalibrationSet, DEFAULT_SIGMA_THRESHOLD,
};
use crate::stacking::combine::config::StackConfig;
use crate::stacking::combine::stack::{StackFrame, stack_images};
use crate::stacking::frame_store::cache_key::DECODE_PINS;
use crate::stacking::progress::ProgressCallback;
use crate::stacking::registration::config::Config as RegistrationConfig;
use crate::stacking::registration::register;
use crate::stacking::registration::resample::{WarpResult, warp};
use crate::stacking::registration::transform::{Transform, WarpTransform};
use crate::stacking::star_detection::config::Config as StarDetectionConfig;
use crate::stacking::star_detection::detector::StarDetector;
use crate::stacking::star_detection::star::Star;
use crate::testing::cfa::make_cfa;
use crate::testing::characterization::snapshot::Snapshot;
use crate::testing::prelude::*;
use common::TempDir;
#[cfg(target_arch = "x86_64")]
use imaginarium::SimdTier;

#[cfg(target_arch = "x86_64")]
use crate::testing::simd_check;
use crate::testing::synthetic::fixtures::star_field;

/// Whether this host computes the pinned digests; another one reports why it skips.
fn pinned_host() -> bool {
    #[cfg(target_arch = "x86_64")]
    return simd_check::runs_here(SimdTier::Avx2Fma);
    #[cfg(not(target_arch = "x86_64"))]
    {
        eprintln!("SKIPPED: the characterization snapshots are pinned on x86_64 only");
        false
    }
}

/// A decode a frame cache can hold, against its pin in [`DECODE_PINS`].
fn assert_decode(decode: &str, snapshot: &Snapshot, pin: &str) {
    assert_eq!(
        snapshot.finish(),
        pin,
        "the {decode} output moved: state the reason with the change and set its DECODE_PINS entry \
         to this digest, which changes DECODE_VERSION so a kept frame cache decodes again"
    );
}

fn assert_snapshot(stage: &str, snapshot: &Snapshot, expected: &str) {
    assert_eq!(
        snapshot.finish(),
        expected,
        "the {stage} snapshot moved: state the reason with the change, or find the regression"
    );
}

fn image_snapshot(snapshot: &mut Snapshot, image: &LinearImage) {
    snapshot.count(image.channels());
    for channel in 0..image.channels() {
        snapshot.f32s(image.channel(channel).pixels());
    }
}

/// The star field every stage after decoding starts from.
fn field() -> LinearImage {
    star_field(Size2us::new(192, 192), 40, 0x5EED).image
}

/// A dither with a little rotation, so warping interpolates every pixel.
fn dither() -> Transform {
    Transform::similarity(DVec2::new(3.25, -2.5), 0.02, 1.0)
}

fn warped(image: &LinearImage, transform: Transform) -> WarpResult {
    warp(
        image,
        &WarpTransform::new(transform),
        &RegistrationConfig::default().warp,
    )
}

fn detect(image: &LinearImage) -> Vec<Star> {
    StarDetector::from_config(StarDetectionConfig::default())
        .unwrap()
        .detect(image)
        .stars
}

#[test]
fn decode_snapshot() {
    if !pinned_host() {
        return;
    }
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/test_resources/full_example.fits"
    );
    let image = LinearImage::from_file(path, &LoadContext::default()).unwrap();
    let mut snapshot = Snapshot::default();
    image_snapshot(&mut snapshot, &image);
    assert_decode("FITS decode", &snapshot, DECODE_PINS.fits_linear);

    // A three-channel float TIFF written from the star field, read back as linear data.
    let scratch = TempDir::new("characterization_decode");
    let size = Size2us::new(48, 40);
    let channels = [0x61, 0x62, 0x63].map(|seed| star_field(size, 10, seed).image);
    let rgb = LinearImage::from_planar_channels(
        ImageDimensions::new(size, 3),
        channels
            .iter()
            .map(|image| image.channel(0).pixels().to_vec()),
    );
    let tiff = scratch.join("field.tiff");
    rgb.save(&tiff).unwrap();
    let image = LinearImage::from_file(&tiff, &LoadContext::default()).unwrap();
    let mut snapshot = Snapshot::default();
    image_snapshot(&mut snapshot, &image);
    assert_decode("float TIFF decode", &snapshot, DECODE_PINS.float_tiff);

    // An RGGB mosaic FITS, read back as its sensor plane.
    let fits = scratch.join("mosaic.fits");
    let mosaic = make_cfa(
        size,
        channels[0].channel(0).pixels().to_vec(),
        CfaType::Bayer(CfaPattern::Rggb),
    );
    save_cfa_fits(&fits, &mosaic).unwrap();
    let cfa = CfaImage::from_file(&fits, &LoadContext::default()).unwrap();
    let mut snapshot = Snapshot::default();
    snapshot.f32s(cfa.data.pixels());
    assert_decode("mosaic FITS decode", &snapshot, DECODE_PINS.fits_cfa);
}

/// A Bayer light through bias, dark and flat masters, then demosaiced.
#[test]
fn calibrate_snapshot() {
    if !pinned_host() {
        return;
    }
    let size = Size2us::new(64, 64);
    let mut rng = TestRng::new(0xCA1B);
    let bias: Vec<f32> = (0..size.pixel_count())
        .map(|_| 0.02 + 0.002 * rng.next_f32())
        .collect();
    let dark: Vec<f32> = bias
        .iter()
        .enumerate()
        .map(|(i, b)| if i % 517 == 3 { 0.6 } else { b + 0.005 })
        .collect();
    let gain: Vec<f32> = (0..size.pixel_count())
        .map(|i| {
            let (x, y) = (
                (i % size.width) as f32 - 32.0,
                (i / size.width) as f32 - 32.0,
            );
            1.0 - 0.25 * (x * x + y * y) / (2.0 * 32.0 * 32.0)
        })
        .collect();
    let flat: Vec<f32> = gain.iter().zip(&bias).map(|(g, b)| 0.5 * g + b).collect();
    let sky = star_field(size, 12, 0x11).image;
    let light: Vec<f32> = sky
        .channel(0)
        .pixels()
        .iter()
        .zip(&gain)
        .zip(&dark)
        .map(|((s, g), d)| s * g + d)
        .collect();

    let bayer = CfaType::Bayer(CfaPattern::Rggb);
    let masters = CalibrationMasters::from_images(
        CalibrationSet {
            dark: Some(make_cfa(size, dark, bayer)),
            flat: Some(make_cfa(size, flat, bayer)),
            bias: Some(make_cfa(size, bias, bayer)),
            flat_dark: None,
        },
        DEFAULT_SIGMA_THRESHOLD,
        CancelToken::never(),
    )
    .unwrap();
    let mut light = make_cfa(size, light, bayer);
    masters.calibrate(&mut light).unwrap();
    let mut snapshot = Snapshot::default();
    snapshot.f32s(light.data.pixels());
    assert_snapshot("calibration", &snapshot, "8cf72cb5dacfe93e");

    let demosaiced = light.demosaic(&CancelToken::never()).unwrap();
    let mut snapshot = Snapshot::default();
    image_snapshot(&mut snapshot, &demosaiced);
    assert_snapshot("demosaic", &snapshot, "39a9fab73c044eef");
}

#[test]
fn detect_snapshot() {
    if !pinned_host() {
        return;
    }
    let stars = detect(&field());
    let mut snapshot = Snapshot::default();
    snapshot.count(stars.len());
    for star in &stars {
        snapshot.f64s(&[star.pos.x, star.pos.y]).f32s(&[
            star.flux,
            star.fwhm,
            star.eccentricity,
            star.snr,
            star.peak,
            star.sharpness,
        ]);
    }
    assert_snapshot("detection", &snapshot, "740dc46b9f2028a8");
}

#[test]
fn register_snapshot() {
    if !pinned_host() {
        return;
    }
    let reference = field();
    let target = warped(&reference, dither()).image;
    let mut config = RegistrationConfig::default();
    config.ransac.seed = Some(0x5EED);
    let result = register(&detect(&reference), &detect(&target), &config).unwrap();
    let mut snapshot = Snapshot::default();
    snapshot
        .f64s(result.transform().matrix())
        .count(result.num_inliers())
        .f64s(&[result.rms_error()]);
    assert_snapshot("registration", &snapshot, "b1511c40e135a0f4");
}

#[test]
fn warp_snapshot() {
    if !pinned_host() {
        return;
    }
    let result = warped(&field(), dither());
    let mut snapshot = Snapshot::default();
    image_snapshot(&mut snapshot, &result.image);
    snapshot
        .f32s(result.coverage.pixels())
        .f32s(result.confidence.pixels());
    assert_snapshot("warp", &snapshot, "0844b150fa4dea5a");
}

/// The field and two dithers of it, stacked with the default configuration.
#[test]
fn combine_snapshot() {
    if !pinned_host() {
        return;
    }
    let reference = field();
    let mut frames = vec![StackFrame::from(reference.clone())];
    for transform in [dither(), Transform::translation(DVec2::new(-1.5, 2.75))] {
        frames.push(StackFrame::registered(
            &reference,
            warped(&reference, transform),
        ));
    }
    let product = stack_images(
        frames,
        StackConfig::default(),
        ProgressCallback::default(),
        CancelToken::never(),
    )
    .unwrap();
    let mut snapshot = Snapshot::default();
    image_snapshot(&mut snapshot, &product.image);
    assert_snapshot("combine", &snapshot, "b59d8cd028469c88");
}

/// Each automatic stretch on a three-channel field, so the color-preserving paths run.
#[test]
fn stretch_snapshot() {
    if !pinned_host() {
        return;
    }
    let size = Size2us::new(96, 96);
    let channels = [0x51, 0x52, 0x53].map(|seed| star_field(size, 20, seed).image);
    let rgb = LinearImage::from_planar_channels(
        ImageDimensions::new(size, 3),
        channels
            .iter()
            .map(|image| image.channel(0).pixels().to_vec()),
    );
    let mut snapshot = Snapshot::default();
    for stretch in [
        Stretch::auto_asinh(),
        Stretch::auto_stf(),
        Stretch::ghs(5.0, 0.0, 0.1),
    ] {
        let mut image = rgb.clone();
        stretch.apply(&mut image).unwrap();
        image_snapshot(&mut snapshot, &image);
    }
    assert_snapshot("stretch", &snapshot, "3dbf371076451ed6");
}

/// The first RAW light of the dataset, decoded to its CFA plane.
#[cfg(feature = "real-data")]
#[test]
fn raw_decode_snapshot() {
    use crate::io::raw::load_raw_cfa;
    use crate::testing::real_data::raw_frames;

    if !pinned_host() {
        return;
    }
    let path = &raw_frames("Lights")[0];
    let cfa = load_raw_cfa(path, &LoadContext::default()).unwrap();
    let mut snapshot = Snapshot::default();
    snapshot.f32s(cfa.data.pixels());
    assert_decode("RAW decode", &snapshot, DECODE_PINS.raw_cfa);
}
