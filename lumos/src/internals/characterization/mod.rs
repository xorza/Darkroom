//! Characterization snapshots: bit-exact digests of each pipeline stage's output on fixed
//! synthetic input, so a rewrite that moves a result names the stage it moved.
//!
//! A snapshot pins what the code does, not what is right. A change that moves one states why;
//! one that moves without a stated reason is a regression until shown otherwise. The digests are
//! those of an `x86_64` host. Every vector Isa computes the same values, but the scalar code calls
//! the platform's libm, which another architecture or OS may round differently, so another host
//! reports the snapshots as skipped rather than failing them.

mod snapshot;

use std::path::PathBuf;

use common::{CancelToken, TempDir};
use glam::DVec2;

use crate::calibration::calibration_masters::CalibrationMasters;
use crate::calibration::calibration_set::CalibrationSet;
use crate::calibration::defect_map::sigma_threshold::SigmaThreshold;
use crate::calibration::master_recipe::MASTER_PIN;
use crate::calibration::master_role::MasterRole;
use crate::calibration::master_stack::MasterStack;
use crate::calibration::subtractor::Subtractor;
use crate::combine::normalization::Normalization;
use crate::combine::stack_config::StackConfig;
use crate::combine::stack_config::weighting::Weighting;
use crate::combine::stack_frame::StackFrame;
use crate::decode::decode_pins::DECODE_PINS;
use crate::frame::frame_stats::FrameStats;
use crate::image::cfa::cfa_image::CfaImage;
use crate::image::cfa::cfa_pattern::CfaPattern;
use crate::image::cfa::cfa_type::CfaType;
use crate::image::cfa::demosaic::xtrans::markesteijn::MarkesteijnPasses;
use crate::image::cfa::mosaic::Mosaic;
use crate::image::channel_layout::ChannelLayout;
use crate::image::image_dimensions::ImageDimensions;
use crate::image::linear_image::LinearImage;
use crate::image_ops::stretch::{ColorMode, Stretch, StretchMethod};
use crate::ingest::ingest_config::IngestConfig;
use crate::internals::cfa::{XTRANS_PATTERN, make_cfa};
use crate::internals::characterization::snapshot::Snapshot;
use crate::internals::synthetic::fixtures::star_field;
use crate::internals::test_rng::TestRng;
use crate::io::fits::cfa;
use crate::io::load_ctx::LoadCtx;
use crate::io::writer;
use crate::progress::progress_monitor::ProgressMonitor;
use crate::registration::registration_config::RegistrationConfig;
use crate::registration::resample::WarpResult;
use crate::registration::resample::warp_config::WarpConfig;
use crate::registration::transform::Transform;
use crate::registration::warp_transform::WarpTransform;
use crate::star_detection::star::Star;
use crate::star_detection::star_detection_config::StarDetectionConfig;
use crate::star_detection::star_detector::StarDetector;
use common::Size2us;

/// Whether this host computes the pinned digests; another one reports why it skips.
fn pinned_host() -> bool {
    let pinned = cfg!(target_arch = "x86_64");
    if !pinned {
        eprintln!("SKIPPED: the characterization snapshots are pinned on x86_64 only");
    }
    pinned
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
    WarpConfig::default().warp(image, &WarpTransform::new(transform))
}

/// A decoded mosaic as a kept cache holds it: its plane, the flags its decoder set, and its
/// quantization σ, so a change to any of the three moves its decode pin.
fn cfa_snapshot(snapshot: &mut Snapshot, cfa: &CfaImage) {
    snapshot.f32s(cfa.data().pixels());
    let flags = cfa.flags().map_or(&[][..], |flags| flags.bytes());
    snapshot.count(flags.len()).bytes(flags);
    snapshot.f32s(&[cfa
        .metadata
        .derived
        .quantization_sigma()
        .unwrap_or(f32::NAN)]);
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
    let image = LoadCtx::default().linear(path).unwrap();
    let mut snapshot = Snapshot::default();
    image_snapshot(&mut snapshot, &image);
    assert_decode("FITS decode", &snapshot, DECODE_PINS.fits_linear);

    // A three-channel float TIFF written from the star field, read back as linear data.
    let scratch = TempDir::new("characterization_decode");
    let size = Size2us::new(48, 40);
    let channels = [0x61, 0x62, 0x63].map(|seed| star_field(size, 10, seed).image);
    let rgb = LinearImage::from_planar_channels(
        ImageDimensions::new(size, ChannelLayout::Rgb),
        channels
            .iter()
            .map(|image| image.channel(0).pixels().to_vec()),
    );
    let tiff = scratch.join("field.tiff");
    writer::raster(&rgb, &tiff).unwrap();
    let image = LoadCtx::default().linear(&tiff).unwrap();
    let mut snapshot = Snapshot::default();
    image_snapshot(&mut snapshot, &image);
    assert_decode("float TIFF decode", &snapshot, DECODE_PINS.float_tiff);

    // An RGGB mosaic FITS, read back as its sensor plane.
    let fits = scratch.join("mosaic.fits");
    let mosaic = make_cfa(
        size,
        channels[0].channel(0).pixels().to_vec(),
        CfaType::Mosaic(Mosaic::Bayer(CfaPattern::Rggb)),
    );
    cfa::save(&fits, &mosaic).unwrap();
    let cfa = LoadCtx::default().cfa(&fits).unwrap();
    let mut snapshot = Snapshot::default();
    cfa_snapshot(&mut snapshot, &cfa);
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

    let bayer = CfaType::Mosaic(Mosaic::Bayer(CfaPattern::Rggb));
    let masters = CalibrationMasters::from_images(
        CalibrationSet {
            dark: Some(make_cfa(size, dark, bayer)),
            flat: Some(make_cfa(size, flat, bayer)),
            bias: Some(make_cfa(size, bias, bayer)),
            flat_dark: None,
        },
        SigmaThreshold::DEFAULT,
        &CancelToken::never(),
    )
    .unwrap();
    let mut light = make_cfa(size, light, bayer);
    masters.calibrate(&mut light).unwrap();
    let mut snapshot = Snapshot::default();
    snapshot.f32s(light.data().pixels());
    assert_snapshot("calibration", &snapshot, "c0cac2b4835d1059");

    let mut snapshot = Snapshot::default();
    let xtrans = make_cfa(
        size,
        light.data().pixels().to_vec(),
        CfaType::Mosaic(Mosaic::XTrans(XTRANS_PATTERN)),
    );
    let demosaiced = light.demosaic_measured(MarkesteijnPasses::One);
    image_snapshot(&mut snapshot, &demosaiced);
    for passes in [MarkesteijnPasses::One, MarkesteijnPasses::Three] {
        let demosaiced = xtrans.clone().demosaic_measured(passes);
        image_snapshot(&mut snapshot, &demosaiced);
    }
    assert_decode("demosaic", &snapshot, DECODE_PINS.demosaic);
}

/// The master stack a cached master is keyed by: a bias of 3 frames, a dark of 3 and a flat of 8
/// with the bias taken from each frame, every role under its preset, stacked from RGGB mosaic FITS
/// frames of 32 × 32 with Gaussian noise, a vignetted flat and a dark with hot pixels.
#[test]
fn master_snapshot() {
    if !pinned_host() {
        return;
    }
    let size = Size2us::new(32, 32);
    let bayer = CfaType::Mosaic(Mosaic::Bayer(CfaPattern::Rggb));
    let scratch = TempDir::new("lumos_master_snapshot");
    let mut rng = TestRng::new(0x3A57);
    let mut write = |name: String, level: &dyn Fn(usize) -> f32, sigma: f32| -> PathBuf {
        let pixels = (0..size.pixel_count())
            .map(|i| level(i) + sigma * rng.next_gaussian_f32())
            .collect();
        let path = scratch.join(name);
        cfa::save(&path, &make_cfa(size, pixels, bayer)).unwrap();
        path
    };
    let vignette = |i: usize| {
        let (x, y) = ((i % 32) as f32 - 15.5, (i / 32) as f32 - 15.5);
        1.0 - 0.3 * (x * x + y * y) / (2.0 * 15.5 * 15.5)
    };
    let bias: Vec<PathBuf> = (0..3)
        .map(|k| write(format!("bias_{k}.fits"), &|_| 0.02, 0.002))
        .collect();
    let darks: Vec<PathBuf> = (0..3)
        .map(|k| {
            write(
                format!("dark_{k}.fits"),
                &|i| if i % 97 == 5 { 0.4 } else { 0.025 },
                0.002,
            )
        })
        .collect();
    let flats: Vec<PathBuf> = (0..8)
        .map(|k| {
            let level = 0.3 + 0.02 * k as f32;
            write(
                format!("flat_{k}.fits"),
                &|i| 0.02 + level * vignette(i),
                0.004,
            )
        })
        .collect();
    let ingest = IngestConfig::with_cache_dir(scratch.join("cache"));
    let stack = |paths: &[PathBuf], role: MasterRole, subtract: &[Subtractor<'_>]| {
        MasterStack {
            paths,
            role,
            config: role.stack_config(),
            ingest: &ingest,
            subtract,
        }
        .run(ProgressMonitor::unattended())
        .unwrap()
        .unwrap()
    };
    let bias_master = stack(&bias, MasterRole::Bias, &[]);
    let dark_master = stack(&darks, MasterRole::Dark, &[]);
    let flat_master = stack(
        &flats,
        MasterRole::Flat,
        &[Subtractor {
            role: MasterRole::Bias,
            master: &bias_master,
        }],
    );
    let mut snapshot = Snapshot::default();
    for master in [&bias_master, &dark_master, &flat_master] {
        cfa_snapshot(&mut snapshot, master);
    }
    assert_eq!(
        snapshot.finish(),
        MASTER_PIN,
        "the master stack moved: state the reason with the change and set MASTER_PIN to this \
         digest, which moves every MasterRecipe so a cached master is stacked again"
    );
}

/// The statistics a kept frame is committed with: the star field as a mono frame, as an RGGB
/// mosaic, and as that mosaic demosaiced, whose noise is the mosaic's.
#[test]
fn frame_stats_snapshot() {
    if !pinned_host() {
        return;
    }
    let mono = field();
    let mosaic = make_cfa(
        Size2us::new(192, 192),
        mono.channel(0).pixels().to_vec(),
        CfaType::Mosaic(Mosaic::Bayer(CfaPattern::Rggb)),
    );
    let demosaiced = mosaic.clone().demosaic_measured(MarkesteijnPasses::One);
    let mut snapshot = Snapshot::default();
    for stats in [
        FrameStats::measure(&mono),
        FrameStats::measure(&mosaic),
        FrameStats::measure(&demosaiced),
    ] {
        snapshot
            .f32s(&stats.medians)
            .f32s(
                &stats
                    .bg
                    .iter()
                    .map(|split| split.sigma())
                    .collect::<Vec<_>>(),
            )
            .f32s(&stats.sky)
            .f32s(&[stats.quantization_sigma.unwrap_or(-1.0)]);
    }
    assert_decode("frame statistics", &snapshot, DECODE_PINS.frame_stats);
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
    assert_snapshot("detection", &snapshot, "fc7b51416f4dd402");
}

#[test]
fn register_snapshot() {
    if !pinned_host() {
        return;
    }
    let reference = field();
    let target = warped(&reference, dither()).image;
    let mut config = RegistrationConfig::default();
    config.ransac.seed = 0x5EED;
    let result = config
        .registrar()
        .unwrap()
        .register(&detect(&reference), &detect(&target))
        .unwrap();
    let mut snapshot = Snapshot::default();
    snapshot
        .f64s(result.transform().matrix())
        .count(result.inlier_count())
        .f64s(&[result.rms_error()]);
    assert_snapshot("registration", &snapshot, "f6beeb64b73e5f9d");
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
    assert_snapshot("warp", &snapshot, "b0e4f2a5dded7f0a");
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
    let product = (StackConfig {
        weighting: Weighting::Equal,
        normalization: Normalization::None,
        ..StackConfig::light()
    })
    .stack_images(frames, ProgressMonitor::unattended())
    .unwrap();
    let mut snapshot = Snapshot::default();
    image_snapshot(&mut snapshot, &product.image);
    assert_snapshot("combine", &snapshot, "c7fffd8295b11ecd");
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
        ImageDimensions::new(size, ChannelLayout::Rgb),
        channels
            .iter()
            .map(|image| image.channel(0).pixels().to_vec()),
    );
    let mut snapshot = Snapshot::default();
    for stretch in [
        Stretch::auto_asinh(),
        Stretch::auto_stf(),
        Stretch {
            method: StretchMethod::Ghs {
                black_point: 0.0,
                d: 5.0,
                b: 0.0,
                sp: 0.1,
                lp: 0.0,
                hp: 1.0,
            },
            color: ColorMode::ColorPreserving,
        },
    ] {
        let mut image = rgb.clone();
        stretch.apply(&mut image).unwrap();
        image_snapshot(&mut snapshot, &image);
    }
    assert_snapshot("stretch", &snapshot, "088d3a703a929f22");
}

/// The first RAW light of the dataset, decoded to its CFA plane — see [`cfa_snapshot`].
#[cfg(feature = "real-data")]
#[test]
fn raw_decode_snapshot() {
    use crate::internals::real_data::raw_frames;
    use crate::io::raw;

    if !pinned_host() {
        return;
    }
    let path = &raw_frames("Lights")[0];
    let cfa = raw::load_cfa(path, &LoadCtx::default()).unwrap();
    let mut snapshot = Snapshot::default();
    cfa_snapshot(&mut snapshot, &cfa);
    assert_decode("RAW decode", &snapshot, DECODE_PINS.raw_cfa);
}
