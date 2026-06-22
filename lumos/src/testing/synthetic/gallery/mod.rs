//! Visual gallery: render every relevant synthetic-data combination to PNG for eyeball
//! verification. These are `#[test] #[ignore]` so they never run in the normal suite; invoke
//! them explicitly to (re)generate the images:
//!
//! ```bash
//! cargo test -p lumos gallery -- --ignored --nocapture
//! ```
//!
//! Output lands under `test_output/synthetic_gallery/` (gitignored). Astronomical frames are
//! mostly dark, so most images use an asinh stretch (`Stretch::Asinh`) to reveal faint stars,
//! background gradients, and noise texture; flat/level images use `Stretch::Linear` to show
//! true values.

use common::test_utils::test_output_path;
use glam::{DVec2, Vec2};
use image::GrayImage;
use imaginarium::Buffer2;
use std::path::PathBuf;

use crate::testing::synthetic::artifacts::add_cosmic_rays;
use crate::testing::synthetic::backgrounds::NebulaConfig;
use crate::testing::synthetic::camera::{BiasField, Camera, FlatField, PsfModel, SensorDefects};
use crate::testing::synthetic::fixtures::{cluster_field, star_field};
use crate::testing::synthetic::observe::{Observation, observe_dithered, render};
use crate::testing::synthetic::patterns::{checkerboard, diagonal_gradient, horizontal_gradient};
use crate::testing::synthetic::scene::{BackgroundField, Scene};

/// Tone mapping applied before writing 8-bit PNG.
#[derive(Debug, Clone, Copy)]
enum Stretch {
    /// Clamp `[0,1]` → `[0,255]`; shows true pixel levels.
    Linear,
    /// asinh auto-stretch between min and max; reveals faint structure.
    Asinh,
}

fn to_bytes(pixels: &[f32], stretch: Stretch) -> Vec<u8> {
    match stretch {
        Stretch::Linear => pixels
            .iter()
            .map(|&p| (p.clamp(0.0, 1.0) * 255.0) as u8)
            .collect(),
        Stretch::Asinh => {
            let lo = pixels.iter().copied().fold(f32::INFINITY, f32::min);
            let hi = pixels.iter().copied().fold(f32::NEG_INFINITY, f32::max);
            let span = (hi - lo).max(1e-10);
            // astropy-style AsinhStretch: y = asinh(x/a) / asinh(1/a), a = soft knee.
            let a = 0.1f32;
            let denom = (1.0 / a).asinh();
            pixels
                .iter()
                .map(|&p| {
                    let x = ((p - lo) / span).clamp(0.0, 1.0);
                    let y = (x / a).asinh() / denom;
                    (y.clamp(0.0, 1.0) * 255.0) as u8
                })
                .collect()
        }
    }
}

/// Save a grayscale frame to `synthetic_gallery/<name>.png`, returning the path.
fn save(pixels: &[f32], width: usize, height: usize, name: &str, stretch: Stretch) -> PathBuf {
    assert_eq!(
        pixels.len(),
        width * height,
        "pixel/dimension mismatch for {name}"
    );
    let path = test_output_path(&format!("synthetic_gallery/{name}.png"));
    GrayImage::from_raw(width as u32, height as u32, to_bytes(pixels, stretch))
        .expect("buffer fits image dimensions")
        .save(&path)
        .expect("write png");
    path
}

/// Render a forward-model frame and save its sensor image.
fn save_frame(scene: &Scene, camera: &Camera, obs: &Observation, name: &str, stretch: Stretch) {
    let frame = render(scene, camera, obs);
    save(
        frame.image.channel(0).pixels(),
        scene.width,
        scene.height,
        name,
        stretch,
    );
}

/// A representative populated star field over `background`.
fn demo_field(width: usize, height: usize, background: BackgroundField, seed: u64) -> Scene {
    Scene::random_field(width, height, 120, (3.0, 250.0), background, 16.0, seed)
}

#[test]
#[ignore = "visual gallery; run with --ignored"]
fn gallery_backgrounds() {
    let (w, h) = (256, 256);
    let cases: [(&str, BackgroundField, Stretch); 6] = [
        (
            "backgrounds/uniform",
            BackgroundField::Uniform { level: 0.1 },
            Stretch::Linear,
        ),
        (
            "backgrounds/gradient_0deg",
            BackgroundField::Gradient {
                start: 0.02,
                end: 0.4,
                angle: 0.0,
            },
            Stretch::Linear,
        ),
        (
            "backgrounds/gradient_45deg",
            BackgroundField::Gradient {
                start: 0.02,
                end: 0.4,
                angle: std::f32::consts::FRAC_PI_4,
            },
            Stretch::Linear,
        ),
        (
            "backgrounds/vignette",
            BackgroundField::Vignette {
                center: 0.3,
                edge: 0.05,
                falloff: 2.0,
            },
            Stretch::Linear,
        ),
        (
            "backgrounds/nebula",
            BackgroundField::Nebula(NebulaConfig::default()),
            Stretch::Asinh,
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
            Stretch::Asinh,
        ),
    ];
    for (name, bg, stretch) in cases {
        save(&bg.render(w, h), w, h, name, stretch);
    }
}

#[test]
#[ignore = "visual gallery; run with --ignored"]
fn gallery_psf_models() {
    let (w, h) = (64, 64);
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
                angle: std::f32::consts::FRAC_PI_4,
            },
        ),
    ];
    for (name, psf) in cases {
        let scene = Scene::single(w, h, DVec2::new(32.0, 32.0), 8.0, dark.clone());
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
            Stretch::Asinh,
        );
    }
}

#[test]
#[ignore = "visual gallery; run with --ignored"]
fn gallery_noise() {
    let (w, h) = (200, 200);
    let flat = Scene {
        width: w,
        height: h,
        sources: vec![],
        background: BackgroundField::Uniform { level: 0.2 },
    };
    // A uniform field: linear stretch makes the noise grain (or its absence) visible.
    save_frame(
        &flat,
        &Camera::ideal(3.0),
        &Observation::reference(1),
        "noise/flat_ideal",
        Stretch::Linear,
    );
    save_frame(
        &flat,
        &Camera::realistic(3.0),
        &Observation::reference(1),
        "noise/flat_realistic",
        Stretch::Linear,
    );

    // A populated field across shot-noise (well depth) and read-noise levels.
    let field = demo_field(w, h, BackgroundField::Uniform { level: 0.05 }, 7);
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
        Stretch::Asinh,
    );
    save_frame(
        &field,
        &well(50_000.0, 3.0),
        &Observation::reference(2),
        "noise/field_well50k",
        Stretch::Asinh,
    );
    save_frame(
        &field,
        &well(5_000.0, 3.0),
        &Observation::reference(2),
        "noise/field_well5k_more_shot",
        Stretch::Asinh,
    );
    save_frame(
        &field,
        &well(50_000.0, 30.0),
        &Observation::reference(2),
        "noise/field_read30_more_read",
        Stretch::Asinh,
    );
}

#[test]
#[ignore = "visual gallery; run with --ignored"]
fn gallery_sensor() {
    let (w, h) = (256, 256);
    // The multiplicative flat map itself.
    let vignette_flat = FlatField {
        vignette: Some((1.0, 0.4, 2.5)),
        channel_gain: [1.0; 3],
    };
    save(
        &vignette_flat.render(w, h, 0),
        w,
        h,
        "sensor/flat_vignette_map",
        Stretch::Linear,
    );

    // A uniform sky seen through that vignette.
    let sky = Scene {
        width: w,
        height: h,
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
        Stretch::Linear,
    );

    // Defects + bias on a star field: hot pixels, a dead pixel block, a bad column.
    let field = demo_field(w, h, BackgroundField::Uniform { level: 0.05 }, 9);
    let defects = SensorDefects {
        hot: (0..40)
            .map(|i| ((i * 53 + 7) % w, (i * 97 + 3) % h, 0.7))
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
        Stretch::Asinh,
    );
}

#[test]
#[ignore = "visual gallery; run with --ignored"]
fn gallery_scenes() {
    let (w, h) = (512, 512);

    let sparse = Scene::random_field(
        w,
        h,
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
        Stretch::Asinh,
    );

    let dense = demo_field(w, h, BackgroundField::Uniform { level: 0.06 }, 2);
    save_frame(
        &dense,
        &Camera::realistic(3.5),
        &Observation::reference(2),
        "scenes/dense_realistic",
        Stretch::Asinh,
    );

    let over_nebula = demo_field(w, h, BackgroundField::Nebula(NebulaConfig::default()), 3);
    save_frame(
        &over_nebula,
        &Camera::realistic(3.5),
        &Observation::reference(3),
        "scenes/over_nebula",
        Stretch::Asinh,
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
        Stretch::Asinh,
    );

    // Saturation: very bright sources clip flat at the well.
    let bright = Scene::random_field(
        w,
        h,
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
        Stretch::Asinh,
    );

    // Cosmic rays peppered onto a realistic field.
    let frame = render(&dense, &Camera::realistic(3.5), &Observation::reference(7));
    let mut pixels = frame.image.channel(0).pixels().to_vec();
    add_cosmic_rays(&mut pixels, w, 60, (0.5, 1.0), 1234);
    save(&pixels, w, h, "scenes/cosmic_rays", Stretch::Asinh);
}

#[test]
#[ignore = "visual gallery; run with --ignored"]
fn gallery_seeing() {
    let (w, h) = (256, 256);
    let field = demo_field(w, h, BackgroundField::Uniform { level: 0.05 }, 11);
    for scale in [1.0f32, 1.5, 2.5] {
        let obs = Observation {
            seeing_scale: scale,
            ..Observation::reference(1)
        };
        let name = format!("seeing/scale_{}", (scale * 10.0) as u32);
        save_frame(&field, &Camera::realistic(3.0), &obs, &name, Stretch::Asinh);
    }
}

#[test]
#[ignore = "visual gallery; run with --ignored"]
fn gallery_dither() {
    let (w, h) = (256, 256);
    let field = demo_field(w, h, BackgroundField::Uniform { level: 0.05 }, 13);
    let dithers = [
        DVec2::new(0.0, 0.0),
        DVec2::new(12.0, -6.0),
        DVec2::new(-8.0, 10.0),
    ];
    let frames = observe_dithered(&field, &Camera::realistic(3.5), &dithers, 1.0, 21);
    for (i, frame) in frames.iter().enumerate() {
        save(
            frame.image.channel(0).pixels(),
            w,
            h,
            &format!("dither/frame_{i}"),
            Stretch::Asinh,
        );
    }
}

#[test]
#[ignore = "visual gallery; run with --ignored"]
fn gallery_patterns() {
    let (w, h) = (256, 256);
    save(
        checkerboard(w, h, 16, 0.1, 0.9).pixels(),
        w,
        h,
        "patterns/checkerboard",
        Stretch::Linear,
    );
    save(
        horizontal_gradient(w, h, 0.0, 1.0).pixels(),
        w,
        h,
        "patterns/horizontal_gradient",
        Stretch::Linear,
    );
    save(
        diagonal_gradient(w, h).pixels(),
        w,
        h,
        "patterns/diagonal_gradient",
        Stretch::Linear,
    );
}

#[test]
#[ignore = "visual gallery; run with --ignored"]
fn gallery_fixtures() {
    // The exact forward-model fields the benchmarks now run on (fixtures::{star_field,
    // cluster_field}), at inspectable sizes.
    let size = 1024;
    save(
        star_field(size, size, 100, 42).image.channel(0).pixels(),
        size,
        size,
        "fixtures/star_field_sparse",
        Stretch::Asinh,
    );
    save(
        star_field(size, size, 1000, 42).image.channel(0).pixels(),
        size,
        size,
        "fixtures/star_field_dense",
        Stretch::Asinh,
    );
    save(
        cluster_field(size, size, 4000, 42)
            .image
            .channel(0)
            .pixels(),
        size,
        size,
        "fixtures/cluster_field",
        Stretch::Asinh,
    );
    save(
        cluster_field(size, size, 15000, 42)
            .image
            .channel(0)
            .pixels(),
        size,
        size,
        "fixtures/cluster_field_dense",
        Stretch::Asinh,
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
