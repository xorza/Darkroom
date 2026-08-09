//! Visual gallery: render every relevant synthetic-data combination to PNG for eyeball
//! verification. These are `#[test] #[ignore]` so they never run in the normal suite; invoke
//! them explicitly to (re)generate the images:
//!
//! ```bash
//! cargo test -p lumos gallery -- --ignored --nocapture
//! ```
//!
//! Output lands under `test_output/synthetic_gallery/` (gitignored). Astronomical frames are
//! mostly dark, so most images use an asinh tone (`ToneMap::Asinh`) to reveal faint stars,
//! background gradients, and noise texture; flat/level images use `ToneMap::Clamp` to show
//! true values.

use std::f32::consts::FRAC_PI_4;
use std::path::PathBuf;

use common::internals::test_output_path;
use glam::{DVec2, Vec2};

use crate::math::size2us::Size2us;
use imaginarium::Buffer2;

use crate::testing::synthetic::artifacts::add_cosmic_rays;
use crate::testing::synthetic::backgrounds::NebulaConfig;
use crate::testing::synthetic::camera::{BiasField, Camera, FlatField, PsfModel, SensorDefects};
use crate::testing::synthetic::fixtures::{cluster_field, star_field};
use crate::testing::synthetic::observe::{Observation, observe_dithered, render};
use crate::testing::synthetic::patterns::{checkerboard, diagonal_gradient, horizontal_gradient};
use crate::testing::synthetic::scene::{BackgroundField, Scene};
use crate::testing::visual::{self, ToneMap};

/// Save a grayscale frame to `synthetic_gallery/<name>.png`, returning the path.
fn save(pixels: &[f32], size: Size2us, name: &str, tone: ToneMap) -> PathBuf {
    assert_eq!(
        pixels.len(),
        size.pixel_count(),
        "pixel/dimension mismatch for {name}"
    );
    let path = test_output_path(&format!("synthetic_gallery/{name}"));
    visual::save(pixels, size, &path, tone);
    visual::output_path(&path)
}

/// Render a forward-model frame and save its sensor image.
fn save_frame(scene: &Scene, camera: &Camera, obs: &Observation, name: &str, tone: ToneMap) {
    let frame = render(scene, camera, obs);
    save(
        frame.image.channel(0).pixels(),
        Size2us::new(scene.size.width, scene.size.height),
        name,
        tone,
    );
}

/// A representative populated star field over `background`.
fn demo_field(size: Size2us, background: BackgroundField, seed: u64) -> Scene {
    Scene::random_field(size, 120, (3.0, 250.0), background, 16.0, seed)
}

#[test]
#[ignore = "visual gallery; run with --ignored"]
fn gallery_backgrounds() {
    let size = Size2us::new(256, 256);
    let cases: [(&str, BackgroundField, ToneMap); 6] = [
        (
            "backgrounds/uniform",
            BackgroundField::Uniform { level: 0.1 },
            ToneMap::Clamp,
        ),
        (
            "backgrounds/gradient_0deg",
            BackgroundField::Gradient {
                start: 0.02,
                end: 0.4,
                angle: 0.0,
            },
            ToneMap::Clamp,
        ),
        (
            "backgrounds/gradient_45deg",
            BackgroundField::Gradient {
                start: 0.02,
                end: 0.4,
                angle: FRAC_PI_4,
            },
            ToneMap::Clamp,
        ),
        (
            "backgrounds/vignette",
            BackgroundField::Vignette {
                center: 0.3,
                edge: 0.05,
                falloff: 2.0,
            },
            ToneMap::Clamp,
        ),
        (
            "backgrounds/nebula",
            BackgroundField::Nebula(NebulaConfig::default()),
            ToneMap::Asinh,
        ),
        (
            "backgrounds/nebula_elongated",
            BackgroundField::Nebula(NebulaConfig {
                center: Vec2::new(0.4, 0.55),
                radius: 0.35,
                amplitude: 0.3,
                softness: 1.5,
                aspect_ratio: 0.45,
                angle: 0.6,
            }),
            ToneMap::Asinh,
        ),
    ];
    for (name, bg, tone) in cases {
        save(&bg.render(size), size, name, tone);
    }
}

#[test]
#[ignore = "visual gallery; run with --ignored"]
fn gallery_psf_models() {
    let size = Size2us::new(64, 64);
    let dark = BackgroundField::Uniform { level: 0.0 };
    let cases: [(&str, PsfModel); 6] = [
        ("psf/gaussian_fwhm3", PsfModel::Gaussian { fwhm: 3.0 }),
        ("psf/gaussian_fwhm6", PsfModel::Gaussian { fwhm: 6.0 }),
        (
            "psf/moffat_b25",
            PsfModel::Moffat {
                fwhm: 4.0,
                beta: 2.5,
            },
        ),
        (
            "psf/moffat_b47",
            PsfModel::Moffat {
                fwhm: 4.0,
                beta: 4.7,
            },
        ),
        (
            "psf/elliptical_e05",
            PsfModel::Elliptical {
                fwhm: 4.0,
                eccentricity: 0.5,
                angle: 0.0,
            },
        ),
        (
            "psf/elliptical_e07_rot",
            PsfModel::Elliptical {
                fwhm: 4.0,
                eccentricity: 0.7,
                angle: FRAC_PI_4,
            },
        ),
    ];
    for (name, psf) in cases {
        let scene = Scene::single(size, DVec2::new(32.0, 32.0), 8.0, dark.clone());
        let camera = Camera {
            psf,
            ..Camera::ideal(4.0)
        };
        // asinh shows the wings (Moffat vs Gaussian) and the elongation.
        save_frame(
            &scene,
            &camera,
            &Observation::reference(1),
            name,
            ToneMap::Asinh,
        );
    }
}

#[test]
#[ignore = "visual gallery; run with --ignored"]
fn gallery_noise() {
    let size = Size2us::new(200, 200);
    let flat = Scene {
        size,
        sources: vec![],
        background: BackgroundField::Uniform { level: 0.2 },
    };
    // A uniform field: linear tone makes the noise grain (or its absence) visible.
    save_frame(
        &flat,
        &Camera::ideal(3.0),
        &Observation::reference(1),
        "noise/flat_ideal",
        ToneMap::Clamp,
    );
    save_frame(
        &flat,
        &Camera::realistic(3.0),
        &Observation::reference(1),
        "noise/flat_realistic",
        ToneMap::Clamp,
    );

    // A populated field across shot-noise (well depth) and read-noise levels.
    let field = demo_field(size, BackgroundField::Uniform { level: 0.05 }, 7);
    let well = |full_well_e: f32, read_noise_e: f32| Camera {
        full_well_e,
        read_noise_e,
        ..Camera::realistic(3.0)
    };
    save_frame(
        &field,
        &Camera::ideal(3.0),
        &Observation::reference(2),
        "noise/field_ideal",
        ToneMap::Asinh,
    );
    save_frame(
        &field,
        &well(50_000.0, 3.0),
        &Observation::reference(2),
        "noise/field_well50k",
        ToneMap::Asinh,
    );
    save_frame(
        &field,
        &well(5_000.0, 3.0),
        &Observation::reference(2),
        "noise/field_well5k_more_shot",
        ToneMap::Asinh,
    );
    save_frame(
        &field,
        &well(50_000.0, 30.0),
        &Observation::reference(2),
        "noise/field_read30_more_read",
        ToneMap::Asinh,
    );
}

#[test]
#[ignore = "visual gallery; run with --ignored"]
fn gallery_sensor() {
    let size = Size2us::new(256, 256);
    // The multiplicative flat map itself.
    let vignette_flat = FlatField {
        vignette: Some((1.0, 0.4, 2.5)),
        channel_gain: [1.0; 3],
    };
    save(
        &vignette_flat.render(size, 0),
        size,
        "sensor/flat_vignette_map",
        ToneMap::Clamp,
    );

    // A uniform sky seen through that vignette.
    let sky = Scene {
        size,
        sources: vec![],
        background: BackgroundField::Uniform { level: 0.3 },
    };
    let vignetted = Camera {
        flat: vignette_flat,
        ..Camera::ideal(3.0)
    };
    save_frame(
        &sky,
        &vignetted,
        &Observation::reference(1),
        "sensor/sky_through_vignette",
        ToneMap::Clamp,
    );

    // Defects + bias on a star field: hot pixels, a dead pixel block, a bad column.
    let field = demo_field(size, BackgroundField::Uniform { level: 0.05 }, 9);
    let defects = SensorDefects {
        hot: (0..40)
            .map(|i| ((i * 53 + 7) % size.width, (i * 97 + 3) % size.height, 0.7))
            .collect(),
        dead: (60..70)
            .flat_map(|x| (60..70).map(move |y| (x, y)))
            .collect(),
    };
    let bias = BiasField {
        offset: 0.04,
        bad_columns: vec![(128, 0.25), (190, 0.15)],
    };
    let camera = Camera {
        defects,
        bias,
        ..Camera::realistic(3.0)
    };
    save_frame(
        &field,
        &camera,
        &Observation::reference(3),
        "sensor/defects_and_bias",
        ToneMap::Asinh,
    );
}

#[test]
#[ignore = "visual gallery; run with --ignored"]
fn gallery_scenes() {
    let size = Size2us::new(512, 512);

    let sparse = Scene::random_field(
        size,
        40,
        (5.0, 250.0),
        BackgroundField::Uniform { level: 0.05 },
        20.0,
        1,
    );
    save_frame(
        &sparse,
        &Camera::ideal(3.5),
        &Observation::reference(1),
        "scenes/sparse_ideal",
        ToneMap::Asinh,
    );

    let dense = demo_field(size, BackgroundField::Uniform { level: 0.06 }, 2);
    save_frame(
        &dense,
        &Camera::realistic(3.5),
        &Observation::reference(2),
        "scenes/dense_realistic",
        ToneMap::Asinh,
    );

    let over_nebula = demo_field(size, BackgroundField::Nebula(NebulaConfig::default()), 3);
    save_frame(
        &over_nebula,
        &Camera::realistic(3.5),
        &Observation::reference(3),
        "scenes/over_nebula",
        ToneMap::Asinh,
    );

    // Tracking error: an elliptical PSF across the whole field.
    let elliptical = Camera {
        psf: PsfModel::Elliptical {
            fwhm: 3.5,
            eccentricity: 0.6,
            angle: 0.5,
        },
        ..Camera::realistic(3.5)
    };
    save_frame(
        &dense,
        &elliptical,
        &Observation::reference(4),
        "scenes/elliptical_tracking_error",
        ToneMap::Asinh,
    );

    // Saturation: very bright sources clip flat at the well.
    let bright = Scene::random_field(
        size,
        25,
        (300.0, 4000.0),
        BackgroundField::Uniform { level: 0.05 },
        20.0,
        5,
    );
    save_frame(
        &bright,
        &Camera::realistic(3.5),
        &Observation::reference(6),
        "scenes/saturated_stars",
        ToneMap::Asinh,
    );

    // Cosmic rays peppered onto a realistic field.
    let frame = render(&dense, &Camera::realistic(3.5), &Observation::reference(7));
    let mut pixels = frame.image.channel(0).pixels().to_vec();
    add_cosmic_rays(&mut pixels, size.width, 60, (0.5, 1.0), 1234);
    save(&pixels, size, "scenes/cosmic_rays", ToneMap::Asinh);
}

#[test]
#[ignore = "visual gallery; run with --ignored"]
fn gallery_seeing() {
    let size = Size2us::new(256, 256);
    let field = demo_field(size, BackgroundField::Uniform { level: 0.05 }, 11);
    for scale in [1.0f32, 1.5, 2.5] {
        let obs = Observation {
            seeing_scale: scale,
            ..Observation::reference(1)
        };
        let name = format!("seeing/scale_{}", (scale * 10.0) as u32);
        save_frame(&field, &Camera::realistic(3.0), &obs, &name, ToneMap::Asinh);
    }
}

#[test]
#[ignore = "visual gallery; run with --ignored"]
fn gallery_dither() {
    let size = Size2us::new(256, 256);
    let field = demo_field(size, BackgroundField::Uniform { level: 0.05 }, 13);
    let dithers = [
        DVec2::new(0.0, 0.0),
        DVec2::new(12.0, -6.0),
        DVec2::new(-8.0, 10.0),
    ];
    let frames = observe_dithered(&field, &Camera::realistic(3.5), &dithers, 1.0, 21);
    for (i, frame) in frames.iter().enumerate() {
        save(
            frame.image.channel(0).pixels(),
            size,
            &format!("dither/frame_{i}"),
            ToneMap::Asinh,
        );
    }
}

#[test]
#[ignore = "visual gallery; run with --ignored"]
fn gallery_patterns() {
    let size = Size2us::new(256, 256);
    save(
        checkerboard(size, 16, 0.1, 0.9).pixels(),
        size,
        "patterns/checkerboard",
        ToneMap::Clamp,
    );
    save(
        horizontal_gradient(size, 0.0, 1.0).pixels(),
        size,
        "patterns/horizontal_gradient",
        ToneMap::Clamp,
    );
    save(
        diagonal_gradient(size).pixels(),
        size,
        "patterns/diagonal_gradient",
        ToneMap::Clamp,
    );
}

#[test]
#[ignore = "visual gallery; run with --ignored"]
fn gallery_fixtures() {
    // The exact forward-model fields the benchmarks now run on (fixtures::{star_field,
    // cluster_field}), at inspectable sizes.
    let size = 1024;
    save(
        star_field(Size2us::new(size, size), 100, 42)
            .image
            .channel(0)
            .pixels(),
        Size2us::new(size, size),
        "fixtures/star_field_sparse",
        ToneMap::Asinh,
    );
    save(
        star_field(Size2us::new(size, size), 1000, 42)
            .image
            .channel(0)
            .pixels(),
        Size2us::new(size, size),
        "fixtures/star_field_dense",
        ToneMap::Asinh,
    );
    save(
        cluster_field(Size2us::new(size, size), 4000, 42)
            .image
            .channel(0)
            .pixels(),
        Size2us::new(size, size),
        "fixtures/cluster_field",
        ToneMap::Asinh,
    );
    save(
        cluster_field(Size2us::new(size, size), 15000, 42)
            .image
            .channel(0)
            .pixels(),
        Size2us::new(size, size),
        "fixtures/cluster_field_dense",
        ToneMap::Asinh,
    );
}

/// Print the gallery directory after generating everything, so the path is easy to find.
#[test]
#[ignore = "visual gallery; run with --ignored"]
fn gallery_print_output_dir() {
    let probe = test_output_path("synthetic_gallery/.probe");
    println!(
        "synthetic gallery directory: {}",
        probe.parent().unwrap().display()
    );
    // Touch a buffer so the dir exists even if run alone.
    let _ = Buffer2::<f32>::new_default(1, 1);
}
