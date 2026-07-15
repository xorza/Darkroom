//! Calibration tests on realistic forward-model frames.
//!
//! The unit tests in `tests.rs` / `defect_map.rs` cover the calibrate/defect logic on uniform
//! `constant_cfa` frames. These exercise it on **non-uniform, noisy, defect-laden** frames built
//! from the CCD equation (`light = bias + dark + flat·signal + noise`): calibration removes a
//! vignette + dark + bias, recovers a star field through a single noisy light, and the
//! `DefectMap` detects injected hot/cold pixels exactly and repairs them.

use crate::stacking::calibration_masters::defect_map::DefectMap;
use crate::testing::TestRng;
use crate::testing::synthetic::camera::Camera;
use crate::testing::synthetic::metrics::{rms_diff, score_rejection};
use crate::testing::synthetic::noise::{add_read_noise, apply_shot_noise};
use crate::testing::synthetic::observe::{Observation, render};
use crate::testing::synthetic::scene::{BackgroundField, Scene};
use crate::testing::{constant_cfa, make_cfa};
use crate::{CalibrationImages, CalibrationMasters, CfaType};
use common::CancelToken;

/// A multiplicative radial vignette (sensor flat-field response).
fn vignette_map(w: usize, h: usize, center: f32, edge: f32, falloff: f32) -> Vec<f32> {
    let (cx, cy) = (w as f32 / 2.0, h as f32 / 2.0);
    let max_r = (cx * cx + cy * cy).sqrt().max(1.0);
    (0..h)
        .flat_map(|y| {
            (0..w).map(move |x| {
                let (dx, dy) = (x as f32 - cx, y as f32 - cy);
                let t = ((dx * dx + dy * dy).sqrt() / max_r).powf(falloff);
                center + (edge - center) * t
            })
        })
        .collect()
}

/// Add physical sensor noise to a normalized signal in place.
fn add_sensor_noise(px: &mut [f32], full_well: f32, read_noise: f32, seed: u64) {
    let mut rng = TestRng::new(seed);
    apply_shot_noise(px, full_well, &mut rng);
    add_read_noise(px, read_noise, full_well, &mut rng);
    for p in px.iter_mut() {
        *p = p.clamp(0.0, 1.0);
    }
}

fn mean(px: &[f32]) -> f32 {
    px.iter().sum::<f32>() / px.len() as f32
}

#[test]
fn calibrate_removes_vignette_dark_and_bias() {
    // A uniformly-lit sky seen through a vignette, plus bias + dark. Calibration must flatten it.
    let (w, h) = (64, 64);
    let (sky, bias, dark) = (0.3f32, 0.05f32, 0.02f32);
    let flat = vignette_map(w, h, 1.0, 0.7, 2.0);

    // light = bias + dark + flat·sky  (noiseless, for an exact assertion).
    let light_px: Vec<f32> = flat.iter().map(|&f| bias + dark + f * sky).collect();
    // Vignetted before calibration: centre noticeably brighter than the corners.
    let pre_max = light_px.iter().cloned().fold(f32::MIN, f32::max);
    let pre_min = light_px.iter().cloned().fold(f32::MAX, f32::min);
    assert!(
        pre_max / pre_min > 1.2,
        "light should be vignetted, {pre_max}/{pre_min}"
    );

    let masters = CalibrationMasters::from_images(
        CalibrationImages {
            dark: Some(constant_cfa(w, h, bias + dark, CfaType::Mono)),
            // Flat frame under uniform illumination: bias + sensor response.
            flat: Some(make_cfa(
                w,
                h,
                flat.iter().map(|&f| bias + f).collect(),
                CfaType::Mono,
            )),
            bias: Some(constant_cfa(w, h, bias, CfaType::Mono)),
            flat_dark: None,
        },
        5.0,
        CancelToken::never(),
    )
    .unwrap();

    let mut light = make_cfa(w, h, light_px, CfaType::Mono);
    masters.calibrate(&mut light);

    // Recovered = sky·mean(flat), spatially flat (vignette divided out).
    let rec = light.data.pixels();
    let rec_mean = mean(rec);
    let rec_max = rec.iter().cloned().fold(f32::MIN, f32::max);
    let rec_min = rec.iter().cloned().fold(f32::MAX, f32::min);
    assert!(
        (rec_mean - sky * mean(&flat)).abs() < 0.005,
        "recovered mean {rec_mean} vs sky·mean(flat) {}",
        sky * mean(&flat)
    );
    assert!(
        (rec_max - rec_min) / rec_mean < 0.02,
        "vignette not removed: max {rec_max} min {rec_min}"
    );
}

#[test]
fn calibrate_recovers_star_field_through_a_noisy_light() {
    let (w, h) = (96, 96);
    let (bias, dark) = (0.05f32, 0.02f32);

    // True signal (sky + stars), noiseless.
    let scene = Scene::random_field(
        w,
        h,
        12,
        (3.0, 9.0),
        BackgroundField::Uniform { level: 0.1 },
        14.0,
        7,
    );
    let signal = render(&scene, &Camera::ideal(4.0), &Observation::reference(0))
        .truth
        .clean;
    let sig = signal.pixels();

    // light = bias + dark + signal + noise (uniform flat → isolates dark/bias subtraction).
    let mut light_px: Vec<f32> = sig.iter().map(|&s| bias + dark + s).collect();
    add_sensor_noise(&mut light_px, 50_000.0, 3.0, 42);

    let masters = CalibrationMasters::from_images(
        CalibrationImages {
            dark: Some(constant_cfa(w, h, bias + dark, CfaType::Mono)),
            flat: Some(constant_cfa(w, h, bias + 1.0, CfaType::Mono)),
            bias: Some(constant_cfa(w, h, bias, CfaType::Mono)),
            flat_dark: None,
        },
        5.0,
        CancelToken::never(),
    )
    .unwrap();

    let mut light = make_cfa(w, h, light_px, CfaType::Mono);
    masters.calibrate(&mut light);

    // Uniform flat (mean 1) → recovered ≈ true signal. The residual is the single-frame shot+read
    // noise floor (~0.0015–0.003 at 50 ke⁻ well); assert it sits in that band — a tighter upper
    // bound than the old 0.01, and a lower bound that catches noise accidentally not applied.
    let err = rms_diff(light.data.pixels(), sig);
    assert!(
        (0.0008..0.005).contains(&err),
        "calibrated residual {err:.5} should sit at the single-frame noise floor"
    );
}

#[test]
fn defect_map_detects_injected_hot_and_cold_pixels() {
    let (w, h) = (64, 64);
    let n = w * h;

    // Hot pixels (bright) injected into an otherwise-uniform dark.
    let hot: [usize; 5] = [100, 517, 1234, 2048, 3900];
    let mut dark_px = vec![0.05f32; n];
    for &i in &hot {
        dark_px[i] = 0.9;
    }
    let dark = make_cfa(w, h, dark_px, CfaType::Mono);
    let map = DefectMap::default()
        .detect_hot(&dark, 5.0, &CancelToken::never())
        .unwrap();
    // The injected set is clean and uniform, so detection must be *exactly* the 5 hot pixels —
    // no spurious flags (precision 1.0) and none missed (recall 1.0).
    let hot_score = score_rejection(&map.hot_indices, &hot);
    assert_eq!(map.hot_indices.len(), 5, "exactly the 5 hot pixels");
    assert_eq!((hot_score.precision, hot_score.recall), (1.0, 1.0));

    // Dead pixels (≈ 0) injected into an otherwise-uniform flat.
    let dead: [usize; 3] = [200, 1700, 3001];
    let mut flat_px = vec![0.8f32; n];
    for &i in &dead {
        flat_px[i] = 0.01;
    }
    let flat = make_cfa(w, h, flat_px, CfaType::Mono);
    let map = DefectMap::default()
        .detect_cold(&flat, &CancelToken::never())
        .unwrap();
    let cold_score = score_rejection(&map.cold_indices, &dead);
    assert_eq!(map.cold_indices.len(), 3, "exactly the 3 dead pixels");
    assert_eq!((cold_score.precision, cold_score.recall), (1.0, 1.0));
}

#[test]
fn hot_detection_sigma_threshold_is_monotonic() {
    // A noisy dark (so MAD is meaningful) with two tiers of hot pixels — moderate and extreme.
    // A lower σ threshold must flag strictly more pixels than a higher one.
    let (w, h) = (64, 64);
    let n = w * h;
    let mut rng = TestRng::new(99);
    let mut dark_px: Vec<f32> = (0..n)
        .map(|_| 0.1 + rng.next_gaussian_f32() * 0.01)
        .collect();
    for &i in &[500usize, 1500, 2500] {
        dark_px[i] = 0.1 + 0.05; // ~5σ
    }
    for &i in &[800usize, 1800, 2800] {
        dark_px[i] = 0.1 + 0.10; // ~10σ
    }
    let dark = make_cfa(w, h, dark_px, CfaType::Mono);
    let lenient = DefectMap::default()
        .detect_hot(&dark, 3.0, &CancelToken::never())
        .unwrap()
        .hot_indices
        .len();
    let strict = DefectMap::default()
        .detect_hot(&dark, 8.0, &CancelToken::never())
        .unwrap()
        .hot_indices
        .len();
    assert!(
        lenient > strict,
        "a lower sigma threshold must flag more hot pixels: σ3 {lenient} vs σ8 {strict}"
    );
}

#[test]
fn defect_correction_replaces_hot_pixels_with_neighbours() {
    let (w, h) = (32, 32);
    let n = w * h;
    let background = 0.2f32;

    let hot: [(usize, usize); 2] = [(10, 10), (20, 15)];
    let mut dark = vec![0.05f32; n];
    for &(x, y) in &hot {
        dark[y * w + x] = 0.9;
    }
    let map = DefectMap::default()
        .detect_hot(
            &make_cfa(w, h, dark, CfaType::Mono),
            5.0,
            &CancelToken::never(),
        )
        .unwrap();

    let mut img_px = vec![background; n];
    for &(x, y) in &hot {
        img_px[y * w + x] = 0.95;
    }
    let mut img = make_cfa(w, h, img_px, CfaType::Mono);
    map.correct(&mut img);

    // Each hot pixel is replaced by its (uniform) neighbourhood; ordinary pixels are untouched.
    let px = img.data.pixels();
    for &(x, y) in &hot {
        assert!(
            (px[y * w + x] - background).abs() < 0.05,
            "hot pixel at ({x},{y}) not corrected: {}",
            px[y * w + x]
        );
    }
    assert!(
        (px[0] - background).abs() < 1e-6,
        "non-defect pixel changed"
    );
}
