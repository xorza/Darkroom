//! Characterization snapshots: each pipeline stage's output on fixed synthetic input, compared
//! with a reference file in `snapshots/`, so a rewrite that moves a result names the stage and
//! the place it moved.
//!
//! A snapshot pins what the code does, not what is right. A change that moves one states why;
//! one that moves without a stated reason is a regression until shown otherwise. The comparison
//! allows a stated tolerance rather than demanding the same bits: Rust leaves `exp`, `ln`, `powf`
//! and the other transcendental functions to the platform's C library, whose results differ by an
//! ulp between glibc, Apple's libm and the MSVC runtime, and between versions of one
//! ([`f64::exp`]). Regression data compared within a tolerance, with a mode that writes it anew,
//! is how GROMACS checks its reference data
//! ([testutils](https://manual.gromacs.org/2022/doxygen/html-lib/group__module__testutils.xhtml)).
//! Refused: bit-exact digests, which fail on a host or a toolchain whose libm rounds differently;
//! a libm of the project's own, which only a need for the same bits on every host would pay for.
//! Each reference a cache keys by — the decodes' and the master stack's — is hashed into that
//! cache's version, so a reference written anew moves it.

mod snapshot;

use std::path::PathBuf;

use common::{CancelToken, TempDir};
use glam::DVec2;
use imaginarium::{ChannelLayout, PlanarDesc};

use crate::calibration::calibration_masters::CalibrationMasters;
use crate::calibration::calibration_set::CalibrationSet;
use crate::calibration::defect_map::sigma_threshold::SigmaThreshold;
use crate::calibration::master_role::MasterRole;
use crate::calibration::master_stack::MasterStack;
use crate::calibration::subtractor::Subtractor;
use crate::combine::normalization::Normalization;
use crate::combine::stack_config::StackConfig;
use crate::combine::stack_config::weighting::Weighting;
use crate::combine::stack_frame::StackFrame;
use crate::frame::frame_stats::FrameStats;
use crate::image::cfa::bayer_pattern::BayerPattern;
use crate::image::cfa::cfa_image::CfaImage;
use crate::image::cfa::cfa_type::CfaType;
use crate::image::cfa::demosaic::xtrans::markesteijn::MarkesteijnPasses;
use crate::image::cfa::mosaic::Mosaic;
use crate::image::flags::pixel_flags::PixelFlags;
use crate::image::flags::quality_flags::QualityFlags;
use crate::image::linear_image::LinearImage;
use crate::image_ops::stretch::{ColorMode, Stretch, StretchMethod};
use crate::ingest::ingest_config::IngestConfig;
use crate::internals::cfa::{XTRANS_PATTERN, make_cfa};
use crate::internals::characterization::snapshot::Snapshot;
use crate::internals::synthetic::fixtures::star_field;
use crate::internals::test_rng::TestRng;
use crate::io::fits::cfa;
use crate::io::load_ctx::LoadCtx;
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

fn image_snapshot(snapshot: &mut Snapshot, name: &str, image: &LinearImage) {
    for channel in 0..image.channel_count() {
        snapshot.plane(&format!("{name} channel {channel}"), image.channel(channel));
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
/// quantization σ, so a change to any of the three moves its reference.
fn cfa_snapshot(snapshot: &mut Snapshot, name: &str, cfa: &CfaImage) {
    let flags = cfa.flags();
    snapshot.plane(&format!("{name} plane"), cfa.data()).digest(
        &format!("{name} flags"),
        flags.map_or(&[][..], PixelFlags::bytes),
    );
    // The digest says the flags moved; each flag's count says which.
    for (flag, flag_name) in QualityFlags::NAMED {
        let count = flags.map_or(0, |flags| flags.count(flag));
        snapshot.count(
            &format!("{name} {flag_name}"),
            usize::try_from(count).expect("a pixel count fits usize"),
        );
    }
    snapshot.f32s(
        &format!("{name} quantization sigma"),
        &[cfa
            .metadata
            .derived
            .quantization_sigma()
            .unwrap_or(f32::NAN)],
    );
}

fn detect(image: &LinearImage) -> Vec<Star> {
    StarDetector::new(StarDetectionConfig::default())
        .detect(image)
        .stars
}

#[test]
fn decode_snapshot() {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/test_resources/full_example.fits"
    );
    let image = LoadCtx::default().linear(path).unwrap();
    let mut snapshot = Snapshot::default();
    image_snapshot(&mut snapshot, "image", &image);
    snapshot.check("fits_linear");

    // A three-channel float TIFF written from the star field, read back as linear data.
    let scratch = TempDir::new("characterization_decode");
    let size = Size2us::new(48, 40);
    let channels = [0x61, 0x62, 0x63].map(|seed| star_field(size, 10, seed).image);
    let rgb = LinearImage::from_planar_channels(
        PlanarDesc::new(size, ChannelLayout::Rgb),
        channels
            .iter()
            .map(|image| image.channel(0).pixels().to_vec()),
    );
    let tiff = scratch.join("field.tiff");
    rgb.write_raster(&tiff).unwrap();
    let image = LoadCtx::default().linear(&tiff).unwrap();
    let mut snapshot = Snapshot::default();
    image_snapshot(&mut snapshot, "image", &image);
    snapshot.check("float_tiff");

    // An RGGB mosaic FITS, read back as its sensor plane.
    let fits = scratch.join("mosaic.fits");
    let mosaic = make_cfa(
        size,
        channels[0].channel(0).pixels().to_vec(),
        CfaType::Mosaic(Mosaic::Bayer(BayerPattern::Rggb)),
    );
    cfa::save(&fits, &mosaic).unwrap();
    let cfa = LoadCtx::default().cfa(&fits).unwrap();
    let mut snapshot = Snapshot::default();
    cfa_snapshot(&mut snapshot, "mosaic", &cfa);
    snapshot.check("fits_cfa");
}

/// A Bayer light through bias, dark and flat masters, then demosaiced.
#[test]
fn calibrate_snapshot() {
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

    let bayer = CfaType::Mosaic(Mosaic::Bayer(BayerPattern::Rggb));
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
    snapshot.plane("light", light.data());
    snapshot.check("calibration");

    let mut snapshot = Snapshot::default();
    let xtrans = make_cfa(
        size,
        light.data().pixels().to_vec(),
        CfaType::Mosaic(Mosaic::XTrans(XTRANS_PATTERN)),
    );
    let demosaiced = light.demosaic_measured(MarkesteijnPasses::One);
    image_snapshot(&mut snapshot, "bayer", &demosaiced);
    for (name, passes) in [
        ("x-trans one pass", MarkesteijnPasses::One),
        ("x-trans three passes", MarkesteijnPasses::Three),
    ] {
        let demosaiced = xtrans.clone().demosaic_measured(passes);
        image_snapshot(&mut snapshot, name, &demosaiced);
    }
    snapshot.check("demosaic");
}

/// The master stack a cached master is keyed by: a bias of 3 frames, a dark of 3 and a flat of 8
/// with the bias taken from each frame, every role under its preset, stacked from RGGB mosaic FITS
/// frames of 32 × 32 with Gaussian noise, a vignetted flat and a dark with hot pixels.
#[test]
fn master_snapshot() {
    let size = Size2us::new(32, 32);
    let bayer = CfaType::Mosaic(Mosaic::Bayer(BayerPattern::Rggb));
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
    for (name, master) in [
        ("bias", &bias_master),
        ("dark", &dark_master),
        ("flat", &flat_master),
    ] {
        cfa_snapshot(&mut snapshot, name, master);
    }
    snapshot.check("master");
}

/// The statistics a kept frame is committed with: the star field as a mono frame, as an RGGB
/// mosaic, and as that mosaic demosaiced, whose noise is the mosaic's.
#[test]
fn frame_stats_snapshot() {
    let mono = field();
    let mosaic = make_cfa(
        Size2us::new(192, 192),
        mono.channel(0).pixels().to_vec(),
        CfaType::Mosaic(Mosaic::Bayer(BayerPattern::Rggb)),
    );
    let demosaiced = mosaic.clone().demosaic_measured(MarkesteijnPasses::One);
    let mut snapshot = Snapshot::default();
    for (name, stats) in [
        ("mono", FrameStats::measure(&mono)),
        ("mosaic", FrameStats::measure(&mosaic)),
        ("demosaiced", FrameStats::measure(&demosaiced)),
    ] {
        let sigmas: Vec<f32> = stats.bg.iter().map(|split| split.sigma()).collect();
        snapshot
            .f32s(&format!("{name} medians"), &stats.medians)
            .f32s(&format!("{name} background sigmas"), &sigmas)
            .f32s(&format!("{name} sky"), &stats.sky)
            .f32s(
                &format!("{name} quantization sigma"),
                &[stats.quantization_sigma.unwrap_or(-1.0)],
            );
    }
    snapshot.check("frame_stats");
}

#[test]
fn detect_snapshot() {
    let stars = detect(&field());
    let column = |field: fn(&Star) -> f32| stars.iter().map(field).collect::<Vec<_>>();
    let mut snapshot = Snapshot::default();
    snapshot
        .count("stars", stars.len())
        .f64s(
            "x",
            &stars.iter().map(|star| star.pos.x).collect::<Vec<_>>(),
        )
        .f64s(
            "y",
            &stars.iter().map(|star| star.pos.y).collect::<Vec<_>>(),
        )
        .f32s("flux", &column(|star| star.flux))
        .f32s("fwhm", &column(|star| star.fwhm))
        .f32s("eccentricity", &column(|star| star.eccentricity))
        .f32s("snr", &column(|star| star.snr))
        .f32s("peak", &column(|star| star.peak))
        .f32s("sharpness", &column(|star| star.sharpness));
    snapshot.check("detection");
}

#[test]
fn register_snapshot() {
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
        .f64s("matrix", result.transform().matrix())
        .count("inliers", result.inlier_count())
        .f64s("rms error", &[result.rms_error()]);
    snapshot.check("registration");
}

#[test]
fn warp_snapshot() {
    let result = warped(&field(), dither());
    let mut snapshot = Snapshot::default();
    image_snapshot(&mut snapshot, "image", &result.image);
    snapshot
        .plane("coverage", &result.coverage)
        .plane("confidence", &result.confidence);
    snapshot.check("warp");
}

/// The field and two dithers of it, stacked with the default configuration.
#[test]
fn combine_snapshot() {
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
    image_snapshot(&mut snapshot, "image", &product.image);
    snapshot.check("combine");
}

/// Each automatic stretch on a three-channel field, so the color-preserving paths run.
#[test]
fn stretch_snapshot() {
    let size = Size2us::new(96, 96);
    let channels = [0x51, 0x52, 0x53].map(|seed| star_field(size, 20, seed).image);
    let rgb = LinearImage::from_planar_channels(
        PlanarDesc::new(size, ChannelLayout::Rgb),
        channels
            .iter()
            .map(|image| image.channel(0).pixels().to_vec()),
    );
    let mut snapshot = Snapshot::default();
    for (name, stretch) in [
        ("asinh", Stretch::auto_asinh()),
        ("stf", Stretch::auto_stf()),
        (
            "ghs",
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
        ),
    ] {
        let mut image = rgb.clone();
        stretch.apply(&mut image).unwrap();
        image_snapshot(&mut snapshot, name, &image);
    }
    snapshot.check("stretch");
}

/// The first RAW light of the dataset, decoded to its CFA plane — see [`cfa_snapshot`].
#[cfg(feature = "real-data")]
#[test]
fn raw_decode_snapshot() {
    use crate::internals::real_data::raw_frames;
    use crate::io::raw;

    let path = &raw_frames("Lights")[0];
    let cfa = raw::load_cfa(path, &LoadCtx::default()).unwrap();
    let mut snapshot = Snapshot::default();
    cfa_snapshot(&mut snapshot, "raw", &cfa);
    snapshot.check("raw_cfa");
}
