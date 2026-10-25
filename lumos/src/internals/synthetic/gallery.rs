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

use common::internals::DEBUG_OUTPUT_VAR;
use common::{Buffer2, internals};
use glam::{DVec2, Vec2};

use crate::image::cfa::cfa_type::CfaType;
use crate::internals::synthetic::artifacts::CosmicRays;
use crate::internals::synthetic::bgs::{Gradient, NebulaConfig, Vignette};
use crate::internals::synthetic::camera::{BiasField, Camera, FlatField, PsfModel, SensorDefects};
use crate::internals::synthetic::fixtures::{cluster_field, star_field};
use crate::internals::synthetic::observe::{Observation, SimFrame, render};
use crate::internals::synthetic::patterns;
use crate::internals::synthetic::scene::{BgField, Placement, Scatter, Scene, TrueSource};
use crate::internals::visual::{self, ToneMap};
use crate::registration::transform::Transform;
use common::{Size2us, Vec2us};

/// Save a grayscale frame to `synthetic_gallery/<name>.png`, returning the path.
fn save(pixels: &Buffer2<f32>, name: &str, tone: ToneMap) -> PathBuf {
    let name = format!("synthetic_gallery/{name}");
    let path = visual::debug_file(&name)
        .unwrap_or_else(|| panic!("the gallery writes images; set {DEBUG_OUTPUT_VAR}=1 to run it"));
    visual::save(pixels, &name, tone);
    path
}

/// Render a forward-model frame and save its sensor image.
fn save_frame(frame: &SimFrame, name: &str, tone: ToneMap) {
    save(frame.image.channel(0), name, tone);
}

/// A representative populated star field over `bg`.
fn demo_field(size: Size2us, bg: BgField, seed: u64) -> Scene {
    Scene::scatter(
        size,
        Scatter {
            count: 120,
            flux: (3.0, 250.0),
            placement: Placement::Uniform { margin: 16.0 },
            seed,
        },
        bg,
    )
}

/// Create a checkerboard pattern.
///
/// Useful for phase correlation and registration tests.
fn checkerboard(size: Size2us, cell_size: usize, [value_a, value_b]: [f32; 2]) -> Buffer2<f32> {
    let mut pixels = vec![0.0f32; size.pixel_count()];
    for y in 0..size.height {
        for x in 0..size.width {
            let checker = ((x / cell_size) + (y / cell_size)) % 2;
            pixels[size.idx_of(Vec2us::new(x, y))] = if checker == 0 { value_a } else { value_b };
        }
    }
    Buffer2::new(size, pixels)
}

/// `base` repeated at each of `dithers`: each translated by its offset, with an independent noise
/// seed derived from `base`'s.
fn dithered<'a>(
    base: &'a Observation,
    dithers: &'a [DVec2],
) -> impl Iterator<Item = Observation> + 'a {
    dithers.iter().enumerate().map(move |(i, &d)| Observation {
        transform: Transform::translation(d),
        seed: base.seed.wrapping_add(i as u64 * 7919),
        ..*base
    })
}

#[test]
#[ignore = "visual gallery; run with --ignored"]
fn gallery_bgs() {
    let size = Size2us::new(256, 256);
    let cases: [(&str, BgField, ToneMap); 6] = [
        (
            "backgrounds/uniform",
            BgField::Uniform { level: 0.1 },
            ToneMap::Clamp,
        ),
        (
            "backgrounds/gradient_0deg",
            BgField::Gradient(Gradient {
                start: 0.02,
                end: 0.4,
                angle: 0.0,
            }),
            ToneMap::Clamp,
        ),
        (
            "backgrounds/gradient_45deg",
            BgField::Gradient(Gradient {
                start: 0.02,
                end: 0.4,
                angle: FRAC_PI_4,
            }),
            ToneMap::Clamp,
        ),
        (
            "backgrounds/vignette",
            BgField::Vignette(Vignette {
                center: 0.3,
                edge: 0.05,
                falloff: 2.0,
            }),
            ToneMap::Clamp,
        ),
        (
            "backgrounds/nebula",
            BgField::Nebula(NebulaConfig::default()),
            ToneMap::Asinh,
        ),
        (
            "backgrounds/nebula_elongated",
            BgField::Nebula(NebulaConfig {
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
        save(&bg.render(size), name, tone);
    }
}

#[test]
#[ignore = "visual gallery; run with --ignored"]
fn gallery_psf_models() {
    let size = Size2us::new(64, 64);
    let dark = BgField::Uniform { level: 0.0 };
    let cases: [(&str, PsfModel); 6] = [
        ("psf/gaussian_fwhm3", PsfModel::gaussian(3.0)),
        ("psf/gaussian_fwhm6", PsfModel::gaussian(6.0)),
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
            PsfModel::Gaussian {
                fwhm: 4.0,
                eccentricity: 0.5,
                angle: 0.0,
            },
        ),
        (
            "psf/elliptical_e07_rot",
            PsfModel::Gaussian {
                fwhm: 4.0,
                eccentricity: 0.7,
                angle: FRAC_PI_4,
            },
        ),
    ];
    for (name, psf) in cases {
        let scene = Scene::single(
            size,
            TrueSource {
                pos: DVec2::new(32.0, 32.0),
                flux: 8.0,
            },
            dark.clone(),
        );
        let camera = Camera {
            psf,
            ..Camera::ideal(4.0)
        };
        // asinh shows the wings (Moffat vs Gaussian) and the elongation.
        save_frame(
            &render(&scene, &camera, &Observation::reference(1)),
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
        bg: BgField::Uniform { level: 0.2 },
    };
    // A uniform field: linear tone makes the noise grain (or its absence) visible.
    save_frame(
        &render(&flat, &Camera::ideal(3.0), &Observation::reference(1)),
        "noise/flat_ideal",
        ToneMap::Clamp,
    );
    save_frame(
        &render(&flat, &Camera::realistic(3.0), &Observation::reference(1)),
        "noise/flat_realistic",
        ToneMap::Clamp,
    );

    // A populated field across shot-noise (well depth) and read-noise levels.
    let field = demo_field(size, BgField::Uniform { level: 0.05 }, 7);
    let well = |full_well_e: f32, read_noise_e: f32| Camera {
        full_well_e,
        read_noise_e,
        ..Camera::realistic(3.0)
    };
    save_frame(
        &render(&field, &Camera::ideal(3.0), &Observation::reference(2)),
        "noise/field_ideal",
        ToneMap::Asinh,
    );
    save_frame(
        &render(&field, &well(50_000.0, 3.0), &Observation::reference(2)),
        "noise/field_well50k",
        ToneMap::Asinh,
    );
    save_frame(
        &render(&field, &well(5_000.0, 3.0), &Observation::reference(2)),
        "noise/field_well5k_more_shot",
        ToneMap::Asinh,
    );
    save_frame(
        &render(&field, &well(50_000.0, 30.0), &Observation::reference(2)),
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
        vignette: Some(Vignette {
            center: 1.0,
            edge: 0.4,
            falloff: 2.5,
        }),
        color_gain: [1.0; 3],
    };
    save(
        &vignette_flat.render(size, &CfaType::Mono),
        "sensor/flat_vignette_map",
        ToneMap::Clamp,
    );

    // A uniform sky seen through that vignette.
    let sky = Scene {
        size,
        sources: vec![],
        bg: BgField::Uniform { level: 0.3 },
    };
    let vignetted = Camera {
        flat: vignette_flat,
        ..Camera::ideal(3.0)
    };
    save_frame(
        &render(&sky, &vignetted, &Observation::reference(1)),
        "sensor/sky_through_vignette",
        ToneMap::Clamp,
    );

    // Defects + bias on a star field: hot pixels, a dead pixel block, a bad column.
    let field = demo_field(size, BgField::Uniform { level: 0.05 }, 9);
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
        bad_cols: vec![(128, 0.25), (190, 0.15)],
    };
    let camera = Camera {
        defects,
        bias,
        ..Camera::realistic(3.0)
    };
    save_frame(
        &render(&field, &camera, &Observation::reference(3)),
        "sensor/defects_and_bias",
        ToneMap::Asinh,
    );
}

#[test]
#[ignore = "visual gallery; run with --ignored"]
fn gallery_scenes() {
    let size = Size2us::new(512, 512);

    let sparse = Scene::scatter(
        size,
        Scatter {
            count: 40,
            flux: (5.0, 250.0),
            placement: Placement::Uniform { margin: 20.0 },
            seed: 1,
        },
        BgField::Uniform { level: 0.05 },
    );
    save_frame(
        &render(&sparse, &Camera::ideal(3.5), &Observation::reference(1)),
        "scenes/sparse_ideal",
        ToneMap::Asinh,
    );

    let dense = demo_field(size, BgField::Uniform { level: 0.06 }, 2);
    save_frame(
        &render(&dense, &Camera::realistic(3.5), &Observation::reference(2)),
        "scenes/dense_realistic",
        ToneMap::Asinh,
    );

    let over_nebula = demo_field(size, BgField::Nebula(NebulaConfig::default()), 3);
    save_frame(
        &render(
            &over_nebula,
            &Camera::realistic(3.5),
            &Observation::reference(3),
        ),
        "scenes/over_nebula",
        ToneMap::Asinh,
    );

    // Tracking error: an elliptical PSF across the whole field.
    let elliptical = Camera {
        psf: PsfModel::Gaussian {
            fwhm: 3.5,
            eccentricity: 0.6,
            angle: 0.5,
        },
        ..Camera::realistic(3.5)
    };
    save_frame(
        &render(&dense, &elliptical, &Observation::reference(4)),
        "scenes/elliptical_tracking_error",
        ToneMap::Asinh,
    );

    // Saturation: very bright sources clip flat at the well.
    let bright = Scene::scatter(
        size,
        Scatter {
            count: 25,
            flux: (300.0, 4000.0),
            placement: Placement::Uniform { margin: 20.0 },
            seed: 5,
        },
        BgField::Uniform { level: 0.05 },
    );
    save_frame(
        &render(&bright, &Camera::realistic(3.5), &Observation::reference(6)),
        "scenes/saturated_stars",
        ToneMap::Asinh,
    );

    // Cosmic rays peppered onto a realistic field.
    let frame = render(&dense, &Camera::realistic(3.5), &Observation::reference(7));
    let mut pixels = frame.image.channel(0).clone();
    CosmicRays {
        count: 60,
        amplitude: 0.5..1.0,
        seed: 1234,
    }
    .add_to(&mut pixels);
    save(&pixels, "scenes/cosmic_rays", ToneMap::Asinh);
}

#[test]
#[ignore = "visual gallery; run with --ignored"]
#[expect(
    clippy::cast_sign_loss,
    reason = "synthetic fixtures are small images with non-negative coordinates"
)]
fn gallery_seeing() {
    let size = Size2us::new(256, 256);
    let field = demo_field(size, BgField::Uniform { level: 0.05 }, 11);
    for scale in [1.0f32, 1.5, 2.5] {
        let obs = Observation {
            seeing_scale: scale,
            ..Observation::reference(1)
        };
        let name = format!("seeing/scale_{}", (scale * 10.0) as u32);
        save_frame(
            &render(&field, &Camera::realistic(3.0), &obs),
            &name,
            ToneMap::Asinh,
        );
    }
}

#[test]
#[ignore = "visual gallery; run with --ignored"]
fn gallery_dither() {
    let size = Size2us::new(256, 256);
    let field = demo_field(size, BgField::Uniform { level: 0.05 }, 13);
    let dithers = [
        DVec2::new(0.0, 0.0),
        DVec2::new(12.0, -6.0),
        DVec2::new(-8.0, 10.0),
    ];
    let camera = Camera::realistic(3.5);
    for (i, obs) in dithered(&Observation::reference(21), &dithers).enumerate() {
        save_frame(
            &render(&field, &camera, &obs),
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
        &checkerboard(size, 16, [0.1, 0.9]),
        "patterns/checkerboard",
        ToneMap::Clamp,
    );
    save(
        &patterns::horizontal_gradient(size, 0.0, 1.0),
        "patterns/horizontal_gradient",
        ToneMap::Clamp,
    );
    save(
        &patterns::diagonal_gradient(size),
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
            .channel(0),
        "fixtures/star_field_sparse",
        ToneMap::Asinh,
    );
    save(
        star_field(Size2us::new(size, size), 1000, 42)
            .image
            .channel(0),
        "fixtures/star_field_dense",
        ToneMap::Asinh,
    );
    save(
        cluster_field(Size2us::new(size, size), 4000, 42)
            .image
            .channel(0),
        "fixtures/cluster_field",
        ToneMap::Asinh,
    );
    save(
        cluster_field(Size2us::new(size, size), 15000, 42)
            .image
            .channel(0),
        "fixtures/cluster_field_dense",
        ToneMap::Asinh,
    );
}

/// Print the gallery directory after generating everything, so the path is easy to find.
#[test]
#[ignore = "visual gallery; run with --ignored"]
fn gallery_print_output_dir() {
    let probe = internals::debug_output_path("synthetic_gallery/.probe")
        .unwrap_or_else(|| panic!("the gallery writes images; set {DEBUG_OUTPUT_VAR}=1 to run it"));
    println!(
        "synthetic gallery directory: {}",
        probe.parent().unwrap().display()
    );
}
