//! Deblending stage tests.
//!
//! The deblender's defining job is to split N known blended sources into exactly N peaks, so
//! these assert the *exact* resolved count and that each true position is recovered — a lower
//! bound (`>= N`) would pass a deblender that under-splits or fragments a single star. A knob
//! sweep pins that the contrast threshold actually controls the split.

use crate::math::fwhm::fwhm_to_sigma;
use crate::stacking::star_detection::config::detection_config::{Deblend, DetectionConfig};
use crate::stacking::star_detection::detector::stages::detect::internals::detect_stars_test;
use crate::stacking::star_detection::tests::stage_effects::{background_estimate, matched_truths};
use crate::testing::prelude::*;
use crate::testing::synthetic::background_map;
use crate::testing::synthetic::sky_field::{Sky, SkyField};
use crate::testing::synthetic::star_profiles::{StarProfile, SyntheticStar};

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

#[test]
fn deblend_resolves_equal_pair_into_exactly_two() {
    let size = Size2us::new(256, 256);
    let fwhm = 4.0;
    let sigma = fwhm_to_sigma(fwhm);
    let sep = fwhm * 2.5;
    let (x1, x2, y) = (128.0 - sep / 2.0, 128.0 + sep / 2.0, 128.0);
    let pixels = field(size, sigma, &[(x1, y, 0.15), (x2, y, 0.15)], 42);
    let background = background_estimate(&pixels);

    let candidates = detect_stars_test(
        &background.residual_of(&pixels),
        &background.sky_noise(),
        &deblend_config(32, 0.005),
    );
    assert_eq!(
        candidates.len(),
        2,
        "equal pair at 2.5 FWHM must split into exactly 2, got {}",
        candidates.len()
    );
    assert_eq!(
        matched_truths(&candidates, &[(x1, y), (x2, y)], sigma),
        2,
        "both true positions must be recovered"
    );
}

#[test]
fn deblend_resolves_chain_of_five() {
    let size = Size2us::new(256, 128);
    let fwhm = 4.0;
    let sigma = fwhm_to_sigma(fwhm);
    let sep = fwhm * 2.5;
    let star_y = 64.0;
    let truths: Vec<(f32, f32)> = (0..5).map(|i| (100.0 + i as f32 * sep, star_y)).collect();
    let stars: Vec<(f32, f32, f32)> = truths.iter().map(|&(x, y)| (x, y, 0.15)).collect();
    let pixels = field(size, sigma, &stars, 42);
    let background = background_estimate(&pixels);

    let candidates = detect_stars_test(
        &background.residual_of(&pixels),
        &background.sky_noise(),
        &deblend_config(32, 0.005),
    );
    assert_eq!(
        candidates.len(),
        5,
        "chain of 5 at 2.5 FWHM must split into exactly 5, got {}",
        candidates.len()
    );
    assert_eq!(
        matched_truths(&candidates, &truths, sigma),
        5,
        "every chain member must be recovered"
    );
}

#[test]
fn deblend_resolves_unequal_pair() {
    let size = Size2us::new(256, 256);
    let fwhm = 4.0;
    let sigma = fwhm_to_sigma(fwhm);
    let sep = fwhm * 2.5;
    let (x1, x2, y) = (128.0 - sep / 2.0, 128.0 + sep / 2.0, 128.0);
    // Bright (~20σ) + faint (~5σ companion).
    let pixels = field(size, sigma, &[(x1, y, 0.20), (x2, y, 0.05)], 42);
    let background = background_estimate(&pixels);

    let candidates = detect_stars_test(
        &background.residual_of(&pixels),
        &background.sky_noise(),
        &deblend_config(32, 0.005),
    );
    assert_eq!(
        candidates.len(),
        2,
        "unequal pair must split into exactly 2, got {}",
        candidates.len()
    );
    assert_eq!(
        matched_truths(&candidates, &[(x1, y), (x2, y)], sigma),
        2,
        "both bright and faint companion must be recovered"
    );
}

#[test]
fn deblend_separation_controls_split() {
    // The separation at which a blended equal pair resolves is the deblender's defining knob:
    // far apart → two peaks, very close → merged into one.
    let size = Size2us::new(256, 256);
    let fwhm = 4.0;
    let sigma = fwhm_to_sigma(fwhm);
    let pair_count = |sep_fwhm: f32| -> usize {
        let sep = fwhm * sep_fwhm;
        let (x1, x2, y) = (128.0 - sep / 2.0, 128.0 + sep / 2.0, 128.0);
        let pixels = field(size, sigma, &[(x1, y, 0.15), (x2, y, 0.15)], 42);
        let background = background_estimate(&pixels);
        detect_stars_test(
            &background.residual_of(&pixels),
            &background.sky_noise(),
            &deblend_config(32, 0.005),
        )
        .len()
    };
    let wide = pair_count(2.5);
    let touching = pair_count(0.5);
    assert_eq!(wide, 2, "a well-separated pair must resolve into 2");
    assert!(
        touching < wide,
        "a near-coincident pair must merge: touching {touching} vs wide {wide}"
    );
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
