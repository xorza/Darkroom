#![expect(
    clippy::cast_sign_loss,
    reason = "test fixtures are small images, with non-negative coordinates and offsets of a few pixels"
)]

use std::f32::consts::SQRT_2;

use super::*;
use crate::drizzle::stack::drizzle_cfa_images;
use crate::ingest::ingest_config::IngestConfig;
use crate::internals::cfa::{XTRANS_PATTERN, make_cfa};
use crate::internals::synthetic::camera::Camera;
use crate::internals::synthetic::metrics;
use crate::internals::synthetic::observe::{Observation, SimFrame, render};
use crate::internals::synthetic::scene::{BackgroundField, Scene};
use crate::io::image::cfa::{CfaImage, CfaType};
use crate::io::raw::demosaic::bayer::CfaPattern;
use crate::io::raw::demosaic::xtrans::markesteijn::MarkesteijnPasses;
use crate::registration::register;
use crate::registration::registration_config::RegistrationConfig;
use crate::registration::transform::{TransformModel, TransformType};
use crate::star_detection::config::Config as DetConfig;
use crate::star_detection::detector::StarDetector;
use common::TempDir;

/// The known scene: colour `colour`'s value at reference pixel `(x, y)`, distinct in every
/// colour and every pixel of a period, and a multiple of 1/64 so every sum below is exact.
fn scene(colour: usize, x: i64, y: i64) -> f32 {
    1.0 + colour as f32 + (x * 31 + y * 17 + colour as i64 * 7).rem_euclid(64) as f32 / 64.0
}

/// A mosaic drizzled at scale 1 and pixfrac 1 under whole-pixel dithers puts each photosite on one
/// output pixel whole, in the channel of its colour alone, so each colour plane reads the scene
/// exactly wherever a photosite of that colour landed, and the fill everywhere else.
///
/// Frame `k`, dithered by `d_k`, sees the scene at reference pixel `p + d_k` through its photosite
/// `p`. Output pixel `o` of channel `c` gathers the frames whose photosite `o − d_k` is on the
/// sensor and of colour `c`: `n` of them, each the scene's own value at weight 1, so their mean
/// is that value exactly, the weight `n`, and the coverage `n` of the frames. RGGB under the four
/// dithers of a period reads red and blue once and green twice inside the first row and column;
/// output pixel (0, 0) is reached only by frame 0's red photosite, so its green and blue hold no
/// weight and take the fill. X-Trans under nine dithers leaves pixels some colour never reached.
///
/// Each set runs twice: with its drizzled frames resident, and spilled to disk under a memory
/// budget of nothing, which writes and maps back every channel's drops.
#[test]
fn a_cfa_drizzle_puts_each_photosite_in_its_colour_alone() {
    const FILL: f32 = -1.0;

    let scratch = TempDir::new("cfa_drizzle_spill");
    let tiers = [
        IngestConfig::default(),
        IngestConfig {
            cache_dir: scratch.path().to_path_buf(),
            memory_override: Some(0),
            ..IngestConfig::default()
        },
    ];

    let size = Size2us::new(18, 18);
    let rggb = CfaType::Bayer(CfaPattern::Rggb);
    let dithers = |side: i64| -> Vec<(i64, i64)> {
        (0..side)
            .flat_map(|dy| (0..side).map(move |dx| (dx, dy)))
            .collect()
    };
    for ((cfa_type, dithers), ingest) in [
        (rggb, dithers(2)),
        (CfaType::XTrans(XTRANS_PATTERN), dithers(3)),
    ]
    .into_iter()
    .flat_map(|set| tiers.iter().map(move |ingest| (set.clone(), ingest)))
    {
        let colour =
            |x: i64, y: i64| usize::from(cfa_type.color_at(Vec2us::new(x as usize, y as usize)));
        let frames = dithers
            .iter()
            .map(|&(dx, dy)| {
                let pixels = (0..size.height as i64)
                    .flat_map(|y| (0..size.width as i64).map(move |x| (x, y)))
                    .map(|(x, y)| scene(colour(x, y), x + dx, y + dy))
                    .collect();
                DrizzleFrame::new(
                    make_cfa(size, pixels, cfa_type),
                    warp_of(Transform::translation(DVec2::new(dx as f64, dy as f64))),
                )
            })
            .collect();
        let config = DrizzleConfig {
            fill_value: FILL,
            ingest: ingest.clone(),
            ..kernel_config(DrizzleKernel::Turbo, 1.0, 1.0)
        };
        let product = drizzle_cfa_images(
            frames,
            &config,
            &plain_stack(),
            ProgressCallback::default(),
            CancelToken::never(),
        )
        .unwrap()
        .product;
        assert_eq!(
            product.report.spilled_frames,
            if ingest.memory_override.is_some() {
                dithers.len() as u64
            } else {
                0
            },
            "{cfa_type:?}: the tier"
        );
        assert_eq!(
            product.cfa_type, None,
            "a drizzle is no stack of sensor frames"
        );
        let weight = product.weight.as_ref().unwrap();
        let Some(Coverage::PerPixel(coverage)) = product.coverage.as_ref() else {
            panic!("{cfa_type:?}: a mosaic's coverage is measured per channel");
        };
        let share = 1.0 / dithers.len() as f32;
        for y in 0..size.height as i64 {
            for x in 0..size.width as i64 {
                for c in 0..3 {
                    let reached = dithers
                        .iter()
                        .filter(|&&(dx, dy)| {
                            let (px, py) = (x - dx, y - dy);
                            px >= 0 && py >= 0 && colour(px, py) == c
                        })
                        .count();
                    let expected = if reached > 0 {
                        (scene(c, x, y), reached as f32, reached as f32 * share)
                    } else {
                        (FILL, 0.0, 0.0)
                    };
                    let (x, y) = (x as usize, y as usize);
                    assert_eq!(
                        (
                            product.image.channel(c)[(x, y)],
                            weight.channel(c)[(x, y)],
                            coverage.channel(c)[(x, y)],
                        ),
                        expected,
                        "{cfa_type:?} {:?} ({x}, {y}) channel {c}: value, weight, coverage",
                        ingest.memory_override
                    );
                }
            }
        }
        if cfa_type == rggb {
            assert_eq!(weight.channel(0)[(0, 0)], 1.0, "red reached (0, 0)");
            assert_eq!(weight.channel(1)[(0, 0)], 0.0, "green never reached (0, 0)");
        }
    }
}

/// The light preset — global normalization, noise weights, σ-clipping — takes a CFA drizzle
/// whole: eight RGGB frames of a sky `level_c·(1 + x/256)` in reference pixels, with levels
/// (0.2, 0.5, 0.3), white noise of σ 0.002 and sub-pixel dithers, drizzled at pixfrac and scale
/// one. A red drop is one pixel wide on a lattice of two, so under a sub-pixel dither it reaches
/// every output pixel beside its photosite, and each frame shares every interior pixel of every
/// colour with the reference, which the frames' pair-by-pair normalization measures over.
///
/// Every output pixel of every colour reads the sky at its reference point to within 6σ — the
/// combine averages several samples, so its noise is below one sample's — plus the slope over a
/// drop's reach: a photosite whose drop covers an output pixel lies within one pixel of it on
/// each axis, where the sky differs by `0.5·√2/256` at most.
#[test]
fn the_light_preset_combines_a_cfa_drizzle() {
    const SIGMA: f32 = 0.002;
    const FRAMES: usize = 8;

    let size = Size2us::new(48, 48);
    let rggb = CfaType::Bayer(CfaPattern::Rggb);
    let levels = [0.2f32, 0.5, 0.3];
    let sky = |c: usize, x: f64| levels[c] * (1.0 + x as f32 / 256.0);
    let mut rng = TestRng::new(5);
    let frames = (0..FRAMES)
        .map(|_| {
            let shift = DVec2::new(rng.next_f64() * 2.0, rng.next_f64() * 2.0);
            let pixels = (0..size.pixel_count())
                .map(|index| {
                    let position = Vec2us::new(index % size.width, index / size.width);
                    let colour = usize::from(rggb.color_at(position));
                    sky(colour, position.x as f64 + shift.x) + SIGMA * rng.next_gaussian_f32()
                })
                .collect();
            DrizzleFrame::new(
                make_cfa(size, pixels, rggb),
                warp_of(Transform::translation(shift)),
            )
        })
        .collect();
    let config = DrizzleConfig {
        fill_value: -1.0,
        ..kernel_config(DrizzleKernel::Square, 1.0, 1.0)
    };
    let product = drizzle_cfa_images(
        frames,
        &config,
        &StackConfig::light(),
        ProgressCallback::default(),
        CancelToken::never(),
    )
    .unwrap()
    .product;
    let bound = 6.0 * SIGMA + 0.5 * SQRT_2 / 256.0;
    let output = product.image.dimensions().size();
    let mut checked = 0;
    for c in 0..3 {
        let plane = product.image.channel(c);
        for y in 0..output.height {
            for x in 0..output.width {
                let value = plane[(x, y)];
                if value == -1.0 {
                    continue;
                }
                checked += 1;
                let expected = sky(c, x as f64);
                assert!(
                    (value - expected).abs() <= bound,
                    "channel {c} ({x}, {y}): {value}, sky {expected}"
                );
            }
        }
    }
    assert!(
        checked > 3 * output.pixel_count() * 9 / 10,
        "the gate filled {} of {} samples",
        3 * output.pixel_count() - checked,
        3 * output.pixel_count()
    );
}

/// A frame of another mosaic pattern is refused before it deposits anything: its photosites
/// would reach other channels at the same positions.
#[test]
fn a_cfa_drizzle_refuses_a_frame_of_another_pattern() {
    let size = Size2us::new(6, 6);
    let frame = |pattern| {
        DrizzleFrame::new(
            make_cfa(size, vec![0.5; size.pixel_count()], CfaType::Bayer(pattern)),
            warp_of(Transform::identity()),
        )
    };
    let error = drizzle_cfa_images(
        vec![frame(CfaPattern::Rggb), frame(CfaPattern::Bggr)],
        &kernel_config(DrizzleKernel::Turbo, 1.0, 1.0),
        &plain_stack(),
        ProgressCallback::default(),
        CancelToken::never(),
    )
    .unwrap_err();
    assert!(
        matches!(
            error,
            DrizzleError::PatternMismatch {
                index: 1,
                expected: Some(CfaType::Bayer(CfaPattern::Rggb)),
                actual: Some(CfaType::Bayer(CfaPattern::Bggr)),
            }
        ),
        "{error:?}"
    );
}

/// The green proxy registers a dithered pair to the transform the demosaiced pair registers to,
/// and both to the truth: an RGGB rendering of one star field, the second frame shifted by
/// (3.37, −2.61), its red and blue photosites at 0.6 and 0.8 of the green's response.
///
/// The bound is the registration's own error: a translation fitted to `n` pairs is their mean
/// offset, off by `σ√2/√n` per axis for a centroid error `σ` per axis, measured here against the
/// render's truth on the proxies' detections; five of those, √2 for the distance. The two fits
/// differ by no more than both errors together.
#[test]
fn the_green_proxy_registers_a_pair_as_the_demosaiced_pair_does() {
    let size = Size2us::new(256, 256);
    let seed = 7;
    let truth = Transform::translation(DVec2::new(3.37, -2.61));
    let scene = Scene::random_field(
        size,
        60,
        (6.0, 16.0),
        BackgroundField::Uniform { level: 0.1 },
        16.0,
        seed,
    );
    let camera = Camera::realistic(4.0);
    let reference = render(&scene, &camera, &Observation::reference(seed));
    let target = render(
        &scene,
        &camera,
        &Observation {
            transform: truth,
            ..Observation::reference(seed + 1)
        },
    );
    let rggb = CfaType::Bayer(CfaPattern::Rggb);
    let mosaic = |frame: &SimFrame| -> CfaImage {
        let gains = [0.6, 1.0, 0.8];
        let pixels = frame
            .image
            .channel(0)
            .iter()
            .enumerate()
            .map(|(index, &value)| {
                let position = Vec2us::new(index % size.width, index / size.width);
                value * gains[usize::from(rggb.color_at(position))]
            })
            .collect();
        make_cfa(size, pixels, rggb)
    };

    let mut config = DetConfig::default();
    config.fwhm.mode = None;
    config.filter.min_snr = 5.0;
    config.detection.sigma_threshold = 3.0;
    let mut detector = StarDetector::from_config(config).unwrap();
    let registration = RegistrationConfig {
        transform_type: TransformModel::Fixed(TransformType::Translation),
        ..Default::default()
    };
    let mut register_on = |reference: LinearImage, target: LinearImage| {
        let stars = [
            detector.detect(&reference).stars,
            detector.detect(&target).stars,
        ];
        let result = register(&stars[0], &stars[1], &registration).unwrap();
        (
            result.transform().apply(DVec2::ZERO),
            result.num_inliers(),
            stars,
        )
    };
    let (proxy, inliers, proxy_stars) = register_on(
        mosaic(&reference).green_proxy(),
        mosaic(&target).green_proxy(),
    );
    let demosaic = |frame: &SimFrame| {
        mosaic(frame)
            .demosaic(MarkesteijnPasses::One, &CancelToken::never())
            .unwrap()
    };
    let (demosaiced, _, _) = register_on(demosaic(&reference), demosaic(&target));

    let sigma =
        metrics::centroid_sigma(&[(&reference, &proxy_stars[0]), (&target, &proxy_stars[1])]);
    let bound = 5.0 * sigma * 2f64.sqrt() / (inliers as f64).sqrt() * 2f64.sqrt();
    let shift = truth.apply(DVec2::ZERO);
    assert!(
        proxy.distance(shift) <= bound,
        "the proxy registers {proxy}, the truth {shift}, bound {bound}"
    );
    assert!(
        demosaiced.distance(shift) <= bound,
        "the demosaiced frames register {demosaiced}, the truth {shift}, bound {bound}"
    );
    assert!(
        proxy.distance(demosaiced) <= 2.0 * bound,
        "the proxy registers {proxy}, the demosaiced frames {demosaiced}"
    );
}
