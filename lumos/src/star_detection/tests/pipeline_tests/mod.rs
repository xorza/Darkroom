//! Full pipeline tests: `StarDetector::detect` on whole forward-model fields, held to the truth
//! that rendered them.

#![expect(
    clippy::cast_sign_loss,
    reason = "test fixtures are small images, with non-negative coordinates and offsets of a few dozen pixels"
)]

use crate::internals::prelude::*;
use crate::internals::synthetic::background_map;
use crate::internals::synthetic::observe::SimFrame;
use crate::internals::visual::report::{DetectionMetrics, save_metrics};
use crate::internals::visual::{ToneMap, save, save_comparison};
use crate::star_detection::config::Config;
use crate::star_detection::config::fwhm_config::FwhmMode;
use crate::star_detection::convolution::{MatchedFilterBuffers, matched_filter};
use crate::star_detection::detector::StarDetector;
use crate::star_detection::detector::internals::saturation_level_of;
use crate::star_detection::tests::{MATCH_RADIUS, near};

mod challenging_tests;
mod standard_tests;

/// How far around a star the grader looks for its island: three FWHM. Even the matched-filtered
/// star — widened to s√2 = 2.4 px and lifted 3× at its peak — is down to `3·e^(−144/(4s²))` =
/// 1.2e-5 of its unfiltered peak there, so what the ring sees above the threshold is something
/// else.
const RING: f64 = 12.0;

/// How far from every source a detection must lie to be no source's at all: two FWHM, as the
/// centroid of a blend no deblender split lies between its stars, up to [`MATCH_RADIUS`] from each.
const BLEND_REACH: f64 = 2.0 * MATCH_RADIUS;

/// Detect `frame` with `config`, save its input, comparison and metrics report under `name`, and
/// hold the detections to the truth: what crowding, faintness and the PSF make of a field as a
/// whole is a matter of degree, so the claims are the ones its truth decides exactly.
///
/// A star is decided when it stands alone on an island: no other source or cosmic ray within
/// [`RING`], the noiseless image the threshold is applied to — the residual, or its matched filter
/// — 2σ under the threshold everywhere on the ring at that radius,
/// and the island clear of the edge margin. Then its footprint joins a neighbour, or sky the
/// background model left, only through a chain of noise excursions past 2σ across the ring, and
/// what it might join that way is fainter than itself, so its peak still leads its region. Of the
/// decided stars:
/// - **every one well above the threshold is found**, within 1 px: a noiseless residual peak `A`
///   of at least `2·(k + 5)`σ at the `k`σ threshold keeps ≥ 12 of its pixels above it through 5σ
///   of noise (`the_threshold_decides_per_star`), and 5σ below the saturation level;
/// - **every saturated one is rejected**: its noiseless peak stands 5σ past the level, so its
///   peak pixel reads saturated, and the filter drops a saturated star whole.
///
/// And **nothing is found away from a true source** at a threshold of 4σ or more, past
/// [`BLEND_REACH`] from every one: sky noise does not make 5 connected pixels past 4σ, and an
/// isolated cosmic ray is rejected (`sharpness_rejects_isolated_cosmic_rays`). Below 4σ, as in the
/// faint scenarios, noise can, and the claim is not made.
///
/// σ is the frame's noise map at the star, the noiseless residual the render's clean signal less
/// the estimated sky. `min_decided` is how many stars the scenario must decide.
fn run_test(name: &str, prefix: &str, frame: &SimFrame, config: &Config, min_decided: usize) {
    let pixels = frame.image.channel(0);
    let size = Size2us::new(pixels.width(), pixels.height());
    let truth = &frame.truth.sources;
    save(
        pixels.pixels(),
        size,
        &format!("synthetic_starfield/{prefix}_{name}_input"),
        ToneMap::Clamp,
    );

    let stars = StarDetector::from_config(config.clone())
        .unwrap()
        .detect(&frame.image)
        .stars;
    let metrics = DetectionMetrics::measure(truth, &stars, MATCH_RADIUS);
    save_comparison(
        pixels.pixels(),
        size,
        truth,
        &stars,
        MATCH_RADIUS as f32,
        &format!("synthetic_starfield/{prefix}_{name}_comparison"),
    );
    save_metrics(
        &metrics,
        &format!("synthetic_starfield/{prefix}_{name}_metrics.txt"),
    );

    let found: Vec<DVec2> = stars.iter().map(|star| star.pos).collect();
    let sources: Vec<DVec2> = truth.iter().map(|source| source.pos).collect();
    let others: Vec<DVec2> = sources
        .iter()
        .copied()
        .chain(
            frame
                .truth
                .cosmic_rays
                .iter()
                .map(|ray| DVec2::new(ray.x as f64, ray.y as f64)),
        )
        .collect();
    let background = background_map::estimate(pixels, &config.background);
    let saturation = saturation_level_of(&frame.image);
    let k = config.detection.sigma_threshold;
    let sigma_at = |x: usize, y: usize| background.noise[(x, y)].max(background.noise_floor);
    let residual_at =
        |x: usize, y: usize| frame.truth.clean[(x, y)] - background.background[(x, y)];
    // What the threshold is applied to: the residual itself, or its matched-filtered image, which
    // lifts a smooth plateau by 2√π·s against noise, twice what it lifts a star's peak by.
    let clean_residual = Buffer2::new(
        size.width,
        size.height,
        (0..size.pixel_count())
            .map(|i| residual_at(i % size.width, i / size.width))
            .collect(),
    );
    let thresholded = match config.fwhm.mode {
        None => clean_residual,
        Some(FwhmMode::Fixed(fwhm)) => {
            let mut output = Buffer2::new_filled(size.width, size.height, 0.0);
            let mut temp = Buffer2::new_filled(size.width, size.height, 0.0);
            matched_filter(
                &clean_residual,
                fwhm,
                config.detection.psf_axis_ratio,
                config.detection.psf_angle,
                &mut MatchedFilterBuffers {
                    output: &mut output,
                    temp: &mut temp,
                },
            );
            output
        }
        Some(FwhmMode::Auto { .. }) => panic!("the grader needs the filter's FWHM fixed"),
    };
    let edge = config.detection.edge_margin as f64 + RING + 1.0;
    let island = |star: DVec2| {
        let inside = star.min_element() >= edge
            && star.x <= size.width as f64 - 1.0 - edge
            && star.y <= size.height as f64 - 1.0 - edge;
        if !inside || near(star, &others, RING) != 1 {
            return false;
        }
        let ring = RING as i64;
        (-ring..=ring).all(|dy| {
            (-ring..=ring).all(|dx| {
                let distance = ((dx * dx + dy * dy) as f64).sqrt();
                if (distance - RING).abs() > 0.5 {
                    return true;
                }
                let (x, y) = (
                    (star.x.round() as i64 + dx) as usize,
                    (star.y.round() as i64 + dy) as usize,
                );
                thresholded[(x, y)] <= (k - 2.0) * sigma_at(x, y)
            })
        })
    };

    let mut decided = 0;
    for &star in sources.iter().filter(|&&star| island(star)) {
        let (x, y) = (star.x.round() as usize, star.y.round() as usize);
        let clean = frame.truth.clean[(x, y)];
        let sigma = sigma_at(x, y);
        if clean >= saturation + 5.0 * sigma {
            assert_eq!(
                near(star, &found, BLEND_REACH),
                0,
                "{name}: the saturated star at {star} was reported"
            );
            decided += 1;
        } else if clean <= saturation - 5.0 * sigma && residual_at(x, y) >= 2.0 * (k + 5.0) * sigma
        {
            assert_eq!(
                near(star, &found, 1.0),
                1,
                "{name}: the star at {star} was not found once within 1 px"
            );
            decided += 1;
        }
    }
    assert!(
        decided >= min_decided,
        "{name}: {decided} stars decided, the scenario holds {min_decided}"
    );

    if k >= 4.0 {
        for star in &found {
            assert!(
                near(*star, &sources, BLEND_REACH) > 0,
                "{name}: the star found at {star} is no source's"
            );
        }
    }
}
