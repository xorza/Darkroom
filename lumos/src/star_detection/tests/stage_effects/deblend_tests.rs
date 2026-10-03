//! Deblending stage tests.
//!
//! The deblender's defining job is to split N known blended sources into exactly N peaks, so
//! these assert the *exact* resolved count and that each true position is recovered — a lower
//! bound (`>= N`) would pass a deblender that under-splits or fragments a single star. Every
//! fixture is checked to be one connected component first, and the contrast and the separation
//! are each shown to move the split.

use crate::internals::prelude::*;
use crate::internals::synthetic::background_map;
use crate::internals::synthetic::sky_field::{Sky, SkyField};
use crate::internals::synthetic::star_profiles::{StarProfile, SyntheticStar};
use crate::math::fwhm::fwhm_to_sigma;
use crate::star_detection::config::detection_config::{Deblend, DetectionConfig};
use crate::star_detection::deblend::region::Region;
use crate::star_detection::detector::stages::detect::internals::{detect_stars_test, detect_test};
use crate::star_detection::tests::stage_effects::{background_estimate, matched_truths};

/// The sky these stage tests detect against: bright enough to matter, quiet enough to detect on.
const DETECTION_SKY: Sky = Sky {
    level: 0.1,
    noise: 0.01,
    clamp: true,
};

/// Render `stars` as `(x, y, amplitude)` on a 0.1 sky with light Gaussian noise (σ 0.01).
fn field(size: Size2us, sigma: f32, stars: &[(f32, f32, f32)], seed: u64) -> Buffer2<f32> {
    let stars: Vec<(Vec2, f32)> = stars
        .iter()
        .map(|&(x, y, amp)| (Vec2::new(x, y), amp))
        .collect();
    SkyField::render(size, DETECTION_SKY, sigma, &stars, seed).pixels
}

fn deblend_config(n_thresholds: usize, min_contrast: f32) -> DetectionConfig {
    DetectionConfig {
        deblend: Deblend::MultiThreshold {
            n_thresholds,
            min_contrast,
        },
        ..Default::default()
    }
}

/// Detect `stars` (σ for a 4 px FWHM) on [`DETECTION_SKY`] at `contrast`. When they come out as
/// more than one region, exactly one component must have been split — the stars shared it, so
/// the split is the deblender's and not the threshold's.
fn deblend_blend(size: Size2us, stars: &[(f32, f32, f32)], contrast: f32) -> Vec<Region> {
    let pixels = field(size, fwhm_to_sigma(4.0), stars, 42);
    let background = background_estimate(&pixels);
    let result = detect_test(
        &background.residual_of(&pixels),
        &background.sky_noise(),
        &deblend_config(32, contrast),
    );
    let expected_splits = usize::from(result.regions.len() > 1);
    assert_eq!(
        result.deblended_components, expected_splits,
        "the stars must be one blended component"
    );
    result.regions
}

// At 1.5 FWHM = 6 px apart (σ = 1.70), the saddle between two stars holds 2·A·e^(−9/5.77) =
// 0.42·A, which keeps a 0.15 pair (0.063) above the 0.04 threshold: one component, two peaks.
// At 2.5 FWHM the saddle is 0.027·A and the pair is two components before any deblending.
const SEPARATION: f32 = 6.0;

#[test]
fn deblend_resolves_equal_pair_into_exactly_two() {
    let (x1, x2, y) = (128.0 - SEPARATION / 2.0, 128.0 + SEPARATION / 2.0, 128.0);
    let candidates = deblend_blend(
        Size2us::new(256, 256),
        &[(x1, y, 0.15), (x2, y, 0.15)],
        0.005,
    );
    assert_eq!(
        candidates.len(),
        2,
        "an equal pair must split into exactly 2"
    );
    assert_eq!(
        matched_truths(&candidates, &[(x1, y), (x2, y)], fwhm_to_sigma(4.0)),
        2,
        "both true positions must be recovered"
    );
}

#[test]
fn deblend_resolves_chain_of_five() {
    let truths: Vec<(f32, f32)> = (0..5)
        .map(|i| (100.0 + i as f32 * SEPARATION, 64.0))
        .collect();
    let stars: Vec<(f32, f32, f32)> = truths.iter().map(|&(x, y)| (x, y, 0.15)).collect();
    let candidates = deblend_blend(Size2us::new(256, 128), &stars, 0.005);
    assert_eq!(
        candidates.len(),
        5,
        "a chain of 5 must split into exactly 5"
    );
    assert_eq!(
        matched_truths(&candidates, &truths, fwhm_to_sigma(4.0)),
        5,
        "every chain member must be recovered"
    );
}

#[test]
fn deblend_resolves_unequal_pair() {
    // A 0.20 star with a 0.10 companion: the saddle holds 0.30·0.21 = 0.063 and the companion
    // peaks at 0.10, above it, so it is a branch of its own. It holds 0.10 / 0.30 = 1/3 of the
    // pair's flux at most: a contrast of 0.005 splits it off, 0.5 keeps the pair whole.
    let (x1, x2, y) = (128.0 - SEPARATION / 2.0, 128.0 + SEPARATION / 2.0, 128.0);
    let stars = [(x1, y, 0.20), (x2, y, 0.10)];

    let split = deblend_blend(Size2us::new(256, 256), &stars, 0.005);
    assert_eq!(split.len(), 2, "the unequal pair must split into exactly 2");
    assert_eq!(
        matched_truths(&split, &[(x1, y), (x2, y)], fwhm_to_sigma(4.0)),
        2,
        "both bright and faint companion must be recovered"
    );
    let whole = deblend_blend(Size2us::new(256, 256), &stars, 0.5);
    assert_eq!(whole.len(), 1, "a 0.5 contrast must keep the pair whole");
}

#[test]
fn deblend_separation_controls_split() {
    // 6 px apart the pair resolves into two; 2 px apart its peaks are one.
    let pair_count = |sep: f32| {
        let (x1, x2, y) = (128.0 - sep / 2.0, 128.0 + sep / 2.0, 128.0);
        deblend_blend(
            Size2us::new(256, 256),
            &[(x1, y, 0.15), (x2, y, 0.15)],
            0.005,
        )
        .len()
    };
    assert_eq!(
        pair_count(SEPARATION),
        2,
        "a resolved pair must split into 2"
    );
    assert_eq!(pair_count(2.0), 1, "a near-coincident pair must merge");
}

/// A group too big for one candidate is still deblended: sixteen 0.3 stars on a 6-px grid form
/// one component of over 500 px — the default `max_area` — holding sixteen peaks. The area bound
/// applies to the regions the deblender makes, each a 6-px cell, so every star is a candidate,
/// under both deblenders.
#[test]
fn a_crowded_group_splits_into_every_star() {
    let truths: Vec<(f32, f32)> = (0..16)
        .map(|i| {
            (
                110.0 + (i % 4) as f32 * SEPARATION,
                110.0 + (i / 4) as f32 * SEPARATION,
            )
        })
        .collect();
    let stars: Vec<(f32, f32, f32)> = truths.iter().map(|&(x, y)| (x, y, 0.3)).collect();
    let pixels = field(Size2us::new(256, 256), fwhm_to_sigma(4.0), &stars, 42);
    let background = background_estimate(&pixels);
    for deblend in [
        DetectionConfig::default().deblend,
        Deblend::MultiThreshold {
            n_thresholds: 32,
            min_contrast: 0.005,
        },
    ] {
        let config = DetectionConfig {
            deblend,
            ..Default::default()
        };
        let result = detect_test(
            &background.residual_of(&pixels),
            &background.sky_noise(),
            &config,
        );
        // One component split — the group, the sky's lone noise pixels staying whole and under
        // the area floor — into sixteen regions that together hold more than `max_area`.
        assert_eq!(result.deblended_components, 1, "{deblend:?}");
        assert_eq!(result.regions.len(), 16, "{deblend:?}");
        let group_area: usize = result.regions.iter().map(|region| region.area).sum();
        assert!(group_area > config.max_area, "{deblend:?}: {group_area} px");
        assert_eq!(
            matched_truths(&result.regions, &truths, fwhm_to_sigma(4.0)),
            16,
            "{deblend:?}: every star of the group"
        );
    }
}

/// Both deblenders decide on the residual, so a sky pedestal under the same blend changes nothing.
///
/// A 0.2 star with a 0.05 one 5 px away (σ 1.5), noiseless, on a sky of 0 and of 0.1 that the
/// map removes exactly. Thresholded at 4 · 0.01, they form one component: the saddle 2.5 px from
/// each holds 0.2·e^(−6.25/4.5) + 0.05·e^(−6.25/4.5) ≈ 0.062. The faint centre is a local maximum
/// of 0.0508 against its neighbours' 0.0457, a quarter of the bright one:
/// - local maxima at prominence 0.3 keeps it whole (0.0508 < 0.06), at 0.2 splits it (≥ 0.04);
/// - multi-threshold splits it at contrast 0.005 and keeps it whole at 0.5.
///
/// Measured on sky-included values, the 0.1 pedestal turned the prominence bar into 0.3 · 0.3 =
/// 0.09 against a 0.15 secondary, so the first case split there and not on a zero sky.
#[test]
fn deblending_does_not_depend_on_the_sky_level() {
    let size = Size2us::new(48, 48);
    let render = |pedestal: f32| {
        let mut pixels = Buffer2::new_filled(size.width, size.height, pedestal);
        for (x, amplitude) in [(22.0, 0.2), (27.0, 0.05)] {
            SyntheticStar::new(
                Vec2::new(x, 24.0),
                amplitude,
                StarProfile::Gaussian { sigma: 1.5 },
            )
            .add_to(&mut pixels);
        }
        pixels
    };
    let peaks = |pedestal: f32, deblend: Deblend| {
        let pixels = render(pedestal);
        let bg = background_map::uniform(size, pedestal, 0.01);
        let config = DetectionConfig {
            deblend,
            min_area: 1,
            ..Default::default()
        };
        let mut peaks: Vec<(usize, usize)> =
            detect_stars_test(&bg.residual_of(&pixels), &bg.sky_noise(), &config)
                .iter()
                .map(|region| (region.peak.x, region.peak.y))
                .collect();
        peaks.sort_unstable();
        peaks
    };

    for (deblend, count) in [
        (
            Deblend::LocalMaxima {
                min_prominence: 0.3,
            },
            1,
        ),
        (
            Deblend::LocalMaxima {
                min_prominence: 0.2,
            },
            2,
        ),
        (
            Deblend::MultiThreshold {
                n_thresholds: 32,
                min_contrast: 0.005,
            },
            2,
        ),
        (
            Deblend::MultiThreshold {
                n_thresholds: 32,
                min_contrast: 0.5,
            },
            1,
        ),
    ] {
        let on_zero = peaks(0.0, deblend);
        assert_eq!(
            on_zero.len(),
            count,
            "{deblend:?} on a zero sky: {on_zero:?}"
        );
        assert_eq!(on_zero[0], (22, 24), "{deblend:?}: the bright peak");
        if count == 2 {
            assert_eq!(on_zero[1], (27, 24), "{deblend:?}: the faint peak");
        }
        assert_eq!(
            peaks(0.1, deblend),
            on_zero,
            "{deblend:?}: the 0.1 sky moved the split"
        );
    }
}
