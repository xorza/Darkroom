//! Cosmic-ray rejection through the whole detector, against the truth that rendered the frame.

#![expect(
    clippy::cast_sign_loss,
    reason = "test fixtures are small images, with non-negative coordinates and offsets of a few dozen pixels"
)]

use crate::internals::init_tracing;
use crate::internals::prelude::*;
use crate::internals::visual::{ToneMap, gray_to_rgb, save_image};
use crate::star_detection::config::Config;
use crate::star_detection::config::filter_config::FilterConfig;
use crate::star_detection::detector::StarDetector;
use crate::star_detection::tests::{Scenario, isolated, near, synthetic_config};
use imaginarium::Color;
use imaginarium::drawing::{draw_circle, draw_cross};

/// The sharpness cut rejects a cosmic ray on its own. A ray is one pixel, or one with 15% bled to
/// each side: its peak holds ≥ 1/1.3 = 0.77 of its 3×3 core, past the 0.7 cut, where a 4-px-FWHM
/// star holds ≤ 0.14. With every other cut that can drop a ray opened — the area floor at 1, the
/// eccentricity bar at 1 and the roundness bar at 2, the FWHM-outlier cut off — the default sharpness cut
/// leaves no isolated ray, and a cut opened to 1.0 keeps every isolated ray below the saturation
/// level (a saturated peak is rejected as such). With the defaults, no isolated ray survives
/// either. Every isolated star survives all three.
#[test]
fn sharpness_rejects_isolated_cosmic_rays() {
    init_tracing();
    let frame = Scenario {
        num_stars: 30,
        cosmic_rays: 15,
        ..Default::default()
    }
    .frame();
    let stars: Vec<DVec2> = frame.truth.sources.iter().map(|s| s.pos).collect();
    let rays: Vec<DVec2> = frame
        .truth
        .cosmic_rays
        .iter()
        .map(|ray| DVec2::new(ray.x as f64, ray.y as f64))
        .collect();
    let pixels = frame.image.channel(0);
    let size = Size2us::new(pixels.width(), pixels.height());
    let (lone_stars, lone_rays) = (isolated(&stars, &rays, size), isolated(&rays, &stars, size));
    let saturation = frame.image.metadata.saturation_level();
    let unsaturated = |ray: &DVec2| pixels[(ray.x as usize, ray.y as usize)] < saturation;
    assert!(
        lone_stars.len() >= 15,
        "{} isolated stars",
        lone_stars.len()
    );
    assert!(
        lone_rays.iter().filter(|ray| unsaturated(ray)).count() >= 5,
        "{} isolated rays, too few unsaturated",
        lone_rays.len()
    );

    let detect = |config: Config| -> Vec<DVec2> {
        StarDetector::from_config(config)
            .unwrap()
            .detect(&frame.image)
            .stars
            .iter()
            .map(|star| star.pos)
            .collect()
    };
    let opened = |max_sharpness| {
        let mut config = synthetic_config();
        config.detection.min_area = 1;
        config.filter.max_eccentricity = 1.0;
        config.filter.max_roundness = 2.0;
        config.filter.max_fwhm_deviation = None;
        config.filter.max_sharpness = max_sharpness;
        config
    };
    let legs = [
        (
            "sharpness cut alone",
            opened(FilterConfig::default().max_sharpness),
            false,
        ),
        ("no cut", opened(1.0), true),
        ("defaults", synthetic_config(), false),
    ];
    for (leg, config, rays_kept) in legs {
        let found = detect(config);
        if leg == "defaults" {
            let mut overlay = gray_to_rgb(pixels.pixels(), size, ToneMap::AutoRange);
            for ray in &rays {
                draw_cross(
                    &mut overlay,
                    ray.as_vec2(),
                    3.0,
                    Color::rgb(1.0, 0.2, 0.2),
                    1.0,
                );
            }
            for star in &stars {
                draw_circle(
                    &mut overlay,
                    star.as_vec2(),
                    8.0,
                    Color::rgb(0.3, 0.3, 1.0),
                    1.0,
                );
            }
            for star in &found {
                draw_circle(&mut overlay, star.as_vec2(), 5.0, Color::GREEN, 1.0);
            }
            save_image(overlay, "synthetic_starfield/stage_cr_rejection_overlay");
        }
        for ray in &lone_rays {
            let expected = usize::from(rays_kept && unsaturated(ray));
            assert_eq!(near(*ray, &found, 1.0), expected, "{leg}: ray at {ray}");
        }
        for star in &lone_stars {
            assert_eq!(near(*star, &found, 1.0), 1, "{leg}: star at {star}");
        }
    }
}
