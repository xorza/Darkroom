//! Deblending stage tests.
//!
//! The deblender's defining job is to split N known blended sources into exactly N peaks, so
//! these assert the *exact* resolved count and that each true position is recovered — a lower
//! bound (`>= N`) would pass a deblender that under-splits or fragments a single star. A knob
//! sweep pins that the contrast threshold actually controls the split.

use crate::math::fwhm_to_sigma;
use crate::stacking::star_detection::config::DetectionConfig;
use crate::stacking::star_detection::detector::stages::detect_test_utils::detect_stars_test;
use crate::stacking::star_detection::synthetic_tests::stage_tests::{
    background_estimate, matched_truths,
};
use crate::testing::TestRng;
use crate::testing::synthetic::star_profiles::render_gaussian_star;
use imaginarium::Buffer2;

/// Render `stars` as `(x, y, amplitude)` on a 0.1 sky with light Gaussian noise (σ 0.01).
fn field(
    width: usize,
    height: usize,
    sigma: f32,
    stars: &[(f32, f32, f32)],
    seed: u64,
) -> Buffer2<f32> {
    let mut pixels = vec![0.1f32; width * height];
    for &(x, y, amp) in stars {
        render_gaussian_star(&mut pixels, width, x, y, sigma, amp);
    }
    let mut rng = TestRng::new(seed);
    for p in &mut pixels {
        *p += rng.next_gaussian_f32() * 0.01;
        *p = p.clamp(0.0, 1.0);
    }
    Buffer2::new(width, height, pixels)
}

fn deblend_config(n_thresholds: usize, min_contrast: f32) -> DetectionConfig {
    DetectionConfig {
        deblend_n_thresholds: n_thresholds,
        deblend_min_contrast: min_contrast,
        ..Default::default()
    }
}

#[test]
fn deblend_resolves_equal_pair_into_exactly_two() {
    let (width, height) = (256, 256);
    let fwhm = 4.0;
    let sigma = fwhm_to_sigma(fwhm);
    let sep = fwhm * 2.5;
    let (x1, x2, y) = (128.0 - sep / 2.0, 128.0 + sep / 2.0, 128.0);
    let pixels = field(width, height, sigma, &[(x1, y, 0.15), (x2, y, 0.15)], 42);
    let background = background_estimate(&pixels);

    let candidates = detect_stars_test(&pixels, &background, &deblend_config(32, 0.005));
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
    let (width, height) = (256, 128);
    let fwhm = 4.0;
    let sigma = fwhm_to_sigma(fwhm);
    let sep = fwhm * 2.5;
    let star_y = 64.0;
    let truths: Vec<(f32, f32)> = (0..5).map(|i| (100.0 + i as f32 * sep, star_y)).collect();
    let stars: Vec<(f32, f32, f32)> = truths.iter().map(|&(x, y)| (x, y, 0.15)).collect();
    let pixels = field(width, height, sigma, &stars, 42);
    let background = background_estimate(&pixels);

    let candidates = detect_stars_test(&pixels, &background, &deblend_config(32, 0.005));
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
    let (width, height) = (256, 256);
    let fwhm = 4.0;
    let sigma = fwhm_to_sigma(fwhm);
    let sep = fwhm * 2.5;
    let (x1, x2, y) = (128.0 - sep / 2.0, 128.0 + sep / 2.0, 128.0);
    // Bright (~20σ) + faint (~5σ companion).
    let pixels = field(width, height, sigma, &[(x1, y, 0.20), (x2, y, 0.05)], 42);
    let background = background_estimate(&pixels);

    let candidates = detect_stars_test(&pixels, &background, &deblend_config(32, 0.005));
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
    let (width, height) = (256, 256);
    let fwhm = 4.0;
    let sigma = fwhm_to_sigma(fwhm);
    let pair_count = |sep_fwhm: f32| -> usize {
        let sep = fwhm * sep_fwhm;
        let (x1, x2, y) = (128.0 - sep / 2.0, 128.0 + sep / 2.0, 128.0);
        let pixels = field(width, height, sigma, &[(x1, y, 0.15), (x2, y, 0.15)], 42);
        let background = background_estimate(&pixels);
        detect_stars_test(&pixels, &background, &deblend_config(32, 0.005)).len()
    };
    let wide = pair_count(2.5);
    let touching = pair_count(0.5);
    assert_eq!(wide, 2, "a well-separated pair must resolve into 2");
    assert!(
        touching < wide,
        "a near-coincident pair must merge: touching {touching} vs wide {wide}"
    );
}
