//! Thresholding and the area cut, through `detect_stars_test`, against the truth that rendered the
//! frame: around every isolated source the candidate count is exact.

#![expect(
    clippy::cast_sign_loss,
    reason = "test fixtures are small images, with non-negative coordinates and offsets of a few dozen pixels"
)]

use crate::internals::init_tracing;
use crate::internals::prelude::*;
use crate::internals::visual::{ToneMap, gray_to_rgb, save_image};
use crate::star_detection::config::detection_config::DetectionConfig;
use crate::star_detection::detector::stages::detect::internals::detect_stars_test;
use crate::star_detection::tests::stage_effects::{background_estimate, peaks};
use crate::star_detection::tests::{ISOLATION, Scenario, isolated, near};
use imaginarium::Color;
use imaginarium::drawing::{draw_circle, draw_cross};

/// How far from a star's centre its candidate's peak pixel can lie. Noiseless, the peak is the
/// nearest pixel, within √2/2; noise can lift a neighbour over it, but every pixel past 2 px holds
/// at most e^(−4/2s²) = 0.5 of the star's peak (s = 1.70 px) against the nearest one's ≥ 0.92 —
/// a gap of ≥ 6σ for the faintest star these tests hold to a candidate (16σ).
const PEAK_RADIUS: f64 = 2.0;

/// Every isolated star of a sparse field is one candidate, peaked at its nearest pixel, and no
/// candidate lies anywhere else: the default threshold over the scenario's bright stars, ≥ 5 px
/// above it, keeps each one whole and the 4σ sky noise below the 5-px area floor.
#[test]
fn every_isolated_star_is_one_candidate() {
    init_tracing();
    let frame = Scenario {
        num_stars: 15,
        ..Default::default()
    }
    .frame();
    let pixels = frame.image.channel(0).clone();
    let size = Size2us::new(pixels.width(), pixels.height());
    let background = background_estimate(&pixels);
    let candidates = detect_stars_test(
        &background.residual_of(&pixels),
        &background.sky_noise(),
        &DetectionConfig::default(),
    );
    let found = peaks(&candidates);
    let stars: Vec<DVec2> = frame.truth.sources.iter().map(|s| s.pos).collect();

    let mut overlay = gray_to_rgb(pixels.pixels(), size, ToneMap::AutoRange);
    for star in &stars {
        draw_circle(
            &mut overlay,
            star.as_vec2(),
            8.0,
            Color::rgb(0.3, 0.3, 1.0),
            1.0,
        );
    }
    for peak in &found {
        draw_cross(&mut overlay, peak.as_vec2(), 3.0, Color::GREEN, 1.0);
    }
    save_image(overlay, "synthetic_starfield/stage_det_sparse_overlay");

    let lone = isolated(&stars, &[], size);
    assert!(lone.len() >= 10, "{} of 15 stars are isolated", lone.len());
    for star in &lone {
        assert_eq!(near(*star, &found, PEAK_RADIUS), 1, "star at {star}");
        assert_eq!(
            near(*star, &found, ISOLATION),
            1,
            "around the star at {star}"
        );
    }
    for peak in &found {
        assert_eq!(
            near(*peak, &stars, ISOLATION),
            1,
            "the candidate at {peak} is no star's"
        );
    }
}

/// Detection is the same when every sample is scaled by 2⁻²⁰: the decoder's span sets a frame's
/// magnitude — a 32-bit integer FITS arrives near 1e-8 — and every threshold is stated in σ, so
/// nothing may hinge on an absolute constant. A power of two scales every sum, median and product
/// of the estimate and the detector exactly, so the candidates are identical — peak, box and area
/// — and each peak value is the native one times 2⁻²⁰, bit for bit.
#[test]
fn detection_is_invariant_to_the_frames_sample_scale() {
    let scale = 2.0f32.powi(-20);
    let pixels = Scenario {
        num_stars: 15,
        ..Default::default()
    }
    .frame()
    .image
    .channel(0)
    .clone();
    let scaled = Buffer2::new(
        pixels.width(),
        pixels.height(),
        pixels.pixels().iter().map(|&v| v * scale).collect(),
    );
    let config = DetectionConfig::default();
    let detect = |pixels: &Buffer2<f32>| {
        let background = background_estimate(pixels);
        let candidates = detect_stars_test(
            &background.residual_of(pixels),
            &background.sky_noise(),
            &config,
        );
        (background.noise_floor, candidates)
    };
    let (native_floor, native) = detect(&pixels);
    let (scaled_floor, scaled) = detect(&scaled);

    assert!(!native.is_empty(), "the fixture detects at its own scale");
    assert_eq!(scaled_floor, native_floor * scale);
    assert_eq!(scaled.len(), native.len());
    for (scaled, native) in scaled.iter().zip(&native) {
        assert_eq!(
            (scaled.peak, scaled.bbox, scaled.area),
            (native.peak, native.bbox, native.area)
        );
        assert_eq!(scaled.peak_value, native.peak_value * scale);
    }
}

/// The threshold decides per star: at `k`σ an isolated star whose noiseless residual peak `A` is at
/// least `2·(k + 5)`σ is a candidate — its footprint above `(k + 5)`σ, where 5σ of noise cannot
/// sink a pixel, spans r² = 2s²·ln 2 = 4 px² around the peak (s = 1.70 px), ≥ 12 pixels against
/// the 5-px area floor — and one whose `A` is at most `(k − 5)`σ is not, as no pixel of it can
/// rise 5σ to the threshold. The noiseless peak is the render's clean signal less the estimated
/// sky; σ is the map's at the peak. Each sweep step moves stars from the first group to the second.
#[test]
fn the_threshold_decides_per_star() {
    let frame = Scenario {
        num_stars: 50,
        flux: (1.0, 10.0),
        full_well_e: 2_000.0,
        ..Default::default()
    }
    .frame();
    let pixels = frame.image.channel(0).clone();
    let size = Size2us::new(pixels.width(), pixels.height());
    let background = background_estimate(&pixels);
    let residual = background.residual_of(&pixels);
    let sky = background.sky_noise();
    let stars: Vec<DVec2> = frame.truth.sources.iter().map(|s| s.pos).collect();
    let lone = isolated(&stars, &[], size);

    let mut previous_detected = usize::MAX;
    for k in [3.0f32, 5.0, 10.0, 20.0, 40.0] {
        let config = DetectionConfig {
            sigma_threshold: k,
            ..Default::default()
        };
        let found = peaks(&detect_stars_test(&residual, &sky, &config));
        let (mut detected, mut missed) = (0, 0);
        for star in &lone {
            let pixel = (star.x.round() as usize, star.y.round() as usize);
            let peak = frame.truth.clean[pixel] - background.background[pixel];
            let sigma = sky.noise[pixel].max(sky.floor);
            if peak >= 2.0 * (k + 5.0) * sigma {
                assert_eq!(near(*star, &found, PEAK_RADIUS), 1, "{k}σ: star at {star}");
                detected += 1;
            } else if peak <= (k - 5.0) * sigma {
                assert_eq!(near(*star, &found, ISOLATION), 0, "{k}σ: star at {star}");
                missed += 1;
            }
        }
        assert!(detected <= previous_detected, "{k}σ: {detected} detected");
        assert!(detected + missed > 0, "{k}σ decides no star");
        previous_detected = detected;
    }
}

/// The area floor tells a cosmic ray from a star: a ray is one pixel, or three where it bled
/// sideways, and a star's footprint is dozens. With the floor at 1 every isolated ray is a
/// candidate; at 4 none is; every isolated star is one in both.
#[test]
fn the_area_floor_drops_cosmic_rays_and_keeps_stars() {
    let frame = Scenario {
        num_stars: 30,
        cosmic_rays: 20,
        ..Default::default()
    }
    .frame();
    let pixels = frame.image.channel(0).clone();
    let size = Size2us::new(pixels.width(), pixels.height());
    let background = background_estimate(&pixels);
    let residual = background.residual_of(&pixels);
    let stars: Vec<DVec2> = frame.truth.sources.iter().map(|s| s.pos).collect();
    let rays: Vec<DVec2> = frame
        .truth
        .cosmic_rays
        .iter()
        .map(|ray| DVec2::new(ray.x as f64, ray.y as f64))
        .collect();
    let (lone_stars, lone_rays) = (isolated(&stars, &rays, size), isolated(&rays, &stars, size));
    assert!(lone_rays.len() >= 5, "{} isolated rays", lone_rays.len());

    for (min_area, rays_kept) in [(1, true), (4, false)] {
        let config = DetectionConfig {
            min_area,
            ..Default::default()
        };
        let found = peaks(&detect_stars_test(
            &residual,
            &background.sky_noise(),
            &config,
        ));
        for ray in &lone_rays {
            assert_eq!(
                near(*ray, &found, PEAK_RADIUS),
                usize::from(rays_kept),
                "area ≥ {min_area}: ray at {ray}"
            );
        }
        for star in &lone_stars {
            assert_eq!(
                near(*star, &found, PEAK_RADIUS),
                1,
                "area ≥ {min_area}: star at {star}"
            );
        }
    }
}
