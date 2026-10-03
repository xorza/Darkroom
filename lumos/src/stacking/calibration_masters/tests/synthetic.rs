//! Calibration tests on realistic forward-model frames.
//!
//! The unit tests in `tests.rs` and `defect_map/tests.rs` cover the calibrate/defect logic on
//! uniform `constant_cfa` frames. These exercise it on **non-uniform, noisy, defect-laden** frames
//! built from the CCD equation (`light = bias + dark + flat·signal + noise`): calibration removes
//! a vignette + dark + bias, recovers a star field through a single noisy light, and the
//! `DefectMap` detects injected hot/cold pixels exactly and repairs them.

use crate::stacking::calibration_masters::defect_map::DefectMap;
use crate::testing::cfa::{constant_cfa, make_cfa};
use crate::testing::prelude::*;
use crate::testing::synthetic::backgrounds::Vignette;
use crate::testing::synthetic::camera::{BiasField, Camera, FlatField, SensorDefects};
use crate::testing::synthetic::metrics::{pixel_stats, rms_diff};
use crate::testing::synthetic::observe::{Observation, render};
use crate::testing::synthetic::patterns;
use crate::testing::synthetic::scene::{BackgroundField, Scene};
use crate::{CalibrationMasters, CalibrationSet, CfaType};

/// A uniformly lit sky seen through a vignette, with bias and dark on top, calibrates to the sky
/// times the flat's mean at every pixel. Noiseless, so the check is the arithmetic's own: the
/// light's sum and subtraction, the flat's sum and subtraction, the divisor `f/mean(f)` and the
/// division each round once, by at most ε of the value: 6ε of `sky · mean(f)`.
#[test]
fn calibrate_removes_vignette_dark_and_bias() {
    let size = Size2us::new(64, 64);
    let (sky, bias, dark) = (0.3f32, 0.05f32, 0.02f32);
    let flat = FlatField {
        vignette: Some(Vignette {
            center: 1.0,
            edge: 0.7,
            falloff: 2.0,
        }),
        ..FlatField::default()
    }
    .render(size, 0);
    let light_px: Vec<f32> = flat.iter().map(|&f| bias + dark + f * sky).collect();
    // Vignetted before calibration: the centre reads 0.07 + 0.3, a corner 0.07 + 0.7·0.3.
    assert_eq!(light_px[32 * 64 + 32], bias + dark + sky);
    assert!(light_px[0] < bias + dark + 0.75 * sky);

    let masters = CalibrationMasters::from_images(
        CalibrationSet {
            dark: Some(constant_cfa(size, bias + dark, CfaType::Mono)),
            flat: Some(make_cfa(
                size,
                flat.iter().map(|&f| bias + f).collect(),
                CfaType::Mono,
            )),
            bias: Some(constant_cfa(size, bias, CfaType::Mono)),
            flat_dark: None,
        },
        5.0,
        &CancelToken::never(),
    )
    .unwrap();
    let mut light = make_cfa(size, light_px, CfaType::Mono);
    masters.calibrate(&mut light).unwrap();

    let expected = f64::from(sky) * pixel_stats(&flat).mean;
    let bound = 6.0 * f64::from(f32::EPSILON) * expected;
    for (i, &v) in light.data.pixels().iter().enumerate() {
        assert!(
            (f64::from(v) - expected).abs() <= bound,
            "pixel {i}: {v} vs sky·mean(flat) {expected}"
        );
    }
}

/// A star field through a single noisy light, with bias and a constant dark: subtracting the dark
/// master and dividing by a unit flat leaves the true signal plus that frame's noise. The noise is
/// the camera's shot and read noise — `√(s/50 000 + (3/50 000)²)` per pixel, 0.0014 at the 0.1 sky
/// and more over the stars — so the residual's RMS sits at that floor, above it only where the
/// stars add shot noise.
#[test]
fn calibrate_recovers_star_field_through_a_noisy_light() {
    let size = Size2us::new(96, 96);
    let (bias, dark) = (0.05f32, 0.02f32);
    let scene = Scene::random_field(
        size,
        12,
        (3.0, 9.0),
        BackgroundField::Uniform { level: 0.1 },
        14.0,
        7,
    );
    let camera = Camera {
        dark_current_e_per_s: 0.0,
        bias: BiasField {
            offset: bias + dark,
            ..BiasField::default()
        },
        ..Camera::realistic(4.0)
    };
    let frame = render(&scene, &camera, &Observation::reference(0));
    let signal = frame.truth.clean.pixels();

    let masters = CalibrationMasters::from_images(
        CalibrationSet {
            dark: Some(constant_cfa(size, bias + dark, CfaType::Mono)),
            flat: Some(constant_cfa(size, bias + 1.0, CfaType::Mono)),
            bias: Some(constant_cfa(size, bias, CfaType::Mono)),
            flat_dark: None,
        },
        5.0,
        &CancelToken::never(),
    )
    .unwrap();
    let mut light = make_cfa(size, frame.image.channel(0).to_vec(), CfaType::Mono);
    masters.calibrate(&mut light).unwrap();

    let err = rms_diff(light.data.pixels(), signal);
    assert!(
        (0.0012..0.005).contains(&err),
        "calibrated residual {err:.5} should sit at the single-frame noise floor"
    );
}

/// The camera's own defect layer: hot pixels spike a dark, dead pixels collect no charge in a
/// flat. Against the read noise of a dark (3 e⁻ of a 50 ke⁻ well, σ 6e-5) a 0.85 spike is over 10⁴σ,
/// and a dead pixel reads ≈ 0 under a 0.8 flat, so detection is exactly the injected sets — no
/// spurious flags and none missed.
#[test]
fn defect_map_detects_the_cameras_hot_and_dead_pixels() {
    let size = Size2us::new(64, 64);
    let hot: [usize; 5] = [100, 517, 1234, 2048, 3900];
    let dead: [usize; 3] = [200, 1700, 3001];
    let camera = Camera {
        dark_current_e_per_s: 0.0,
        bias: BiasField {
            offset: 0.05,
            ..BiasField::default()
        },
        defects: SensorDefects {
            hot: hot.map(|i| (i % 64, i / 64, 0.85)).to_vec(),
            dead: dead.map(|i| (i % 64, i / 64)).to_vec(),
        },
        ..Camera::realistic(4.0)
    };
    let expose = |level: f32, seed: u64| {
        let scene = Scene::random_field(
            size,
            0,
            (1.0, 1.0),
            BackgroundField::Uniform { level },
            0.0,
            seed,
        );
        let frame = render(&scene, &camera, &Observation::reference(seed));
        make_cfa(
            size,
            frame.image.channel(0).pixels().to_vec(),
            CfaType::Mono,
        )
    };

    let dark = expose(0.0, 1);
    let map = DefectMap::new(dark.size())
        .detect_hot(&dark, 5.0, &CancelToken::never())
        .unwrap();
    assert_eq!(map.hot_indices(), hot);

    let flat = expose(0.8, 2);
    let map = DefectMap::new(flat.size())
        .detect_cold(&flat, &CancelToken::never())
        .unwrap();
    assert_eq!(map.cold_indices(), dead);
}

/// The σ threshold decides which tiers are hot. On a dark of 0.1 with σ 0.01 noise, three pixels
/// at 5σ and three at 10σ: at 8σ exactly the three 10σ pixels pass, and at 3σ all six do — plus
/// any noise pixel past 3σ, of which a 4096-pixel Gaussian field has about five.
#[test]
fn hot_detection_sigma_threshold_picks_its_tier() {
    let size = Size2us::new(64, 64);
    let mut dark_px = vec![0.1f32; size.pixel_count()];
    patterns::add_gaussian_noise(&mut dark_px, 0.01, 99);
    let moderate = [500usize, 1500, 2500];
    let extreme = [800usize, 1800, 2800];
    for &i in &moderate {
        dark_px[i] = 0.1 + 0.05;
    }
    for &i in &extreme {
        dark_px[i] = 0.1 + 0.10;
    }
    let dark = make_cfa(size, dark_px, CfaType::Mono);
    let hot_at = |sigma| {
        DefectMap::new(dark.size())
            .detect_hot(&dark, sigma, &CancelToken::never())
            .unwrap()
            .hot_indices()
            .to_vec()
    };
    assert_eq!(hot_at(8.0), extreme);
    let lenient = hot_at(3.0);
    for i in moderate.iter().chain(&extreme) {
        assert!(lenient.contains(i), "σ 3 keeps {i}: {lenient:?}");
    }
}

#[test]
fn defect_correction_replaces_hot_pixels_with_neighbours() {
    let size = Size2us::new(32, 32);
    let n = size.pixel_count();
    let background = 0.2f32;

    let hot: [(usize, usize); 2] = [(10, 10), (20, 15)];
    let mut dark = vec![0.05f32; n];
    for &(x, y) in &hot {
        dark[size.index_of(Vec2us::new(x, y))] = 0.9;
    }
    let map = DefectMap::new(size)
        .detect_hot(
            &make_cfa(size, dark, CfaType::Mono),
            5.0,
            &CancelToken::never(),
        )
        .unwrap();

    let mut img_px = vec![background; n];
    for &(x, y) in &hot {
        img_px[size.index_of(Vec2us::new(x, y))] = 0.95;
    }
    let mut img = make_cfa(size, img_px, CfaType::Mono);
    map.correct(&mut img);

    // Each hot pixel takes its uniform neighbourhood's value; no other pixel changes.
    assert!(img.data.pixels().iter().all(|&v| v == background));
}
