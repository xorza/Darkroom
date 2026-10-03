#[cfg(feature = "real-data")]
mod real_data;

use crate::image_ops::denoise::{Denoise, Threshold};
use crate::testing::images::{gray_image as gray, rgb_image as rgb};
use crate::testing::prelude::*;
use crate::testing::synthetic::metrics::pixel_stats;
use crate::testing::synthetic::patterns;

/// A flat `bg` with white Gaussian noise of `sigma`.
fn noisy(size: Size2us, bg: f32, sigma: f32, seed: u64) -> Vec<f32> {
    let mut px = vec![bg; size.pixel_count()];
    patterns::add_gaussian_noise(&mut px, sigma, seed);
    px
}

#[test]
fn threshold_apply_hand_computed() {
    // Hard: keep |w| >= t, else 0.
    assert_eq!(Threshold::Hard.apply(0.3, 0.2), 0.3);
    assert_eq!(Threshold::Hard.apply(-0.3, 0.2), -0.3);
    assert_eq!(Threshold::Hard.apply(0.1, 0.2), 0.0);
    assert_eq!(Threshold::Hard.apply(0.2, 0.2), 0.2); // boundary is kept (>=)
    // Soft: sign(w) * max(|w| - t, 0).
    assert_eq!(Threshold::Soft.apply(0.3, 0.2), 0.3 - 0.2);
    assert_eq!(Threshold::Soft.apply(-0.3, 0.2), -(0.3 - 0.2));
    assert_eq!(Threshold::Soft.apply(0.1, 0.2), 0.0);
}

#[test]
fn denoise_reduces_white_noise_and_preserves_mean() {
    let size = Size2us::new(128, 128);
    let (bg, sigma) = (0.5, 0.05);
    let px = noisy(size, bg, sigma, 12345);
    let in_std = pixel_stats(&px).std as f32;

    let mut img = gray(size, px);
    Denoise::default().apply(&mut img).unwrap();
    let out = img.channel(0).to_vec();

    let out_std = pixel_stats(&out).std as f32;
    assert!(
        out_std < 0.6 * in_std,
        "white noise reduced: out_std {out_std} vs in_std {in_std}"
    );
    // What denoising removes is thresholded white noise, zero-mean: over 16 384 pixels its mean
    // is of order σ/√n = 4e-4, and the input's own mean sits that close to the background too.
    // 2e-3 is five of those.
    assert_close!(pixel_stats(&out).mean, bg, 2e-3, "DC preserved");
}

#[test]
fn higher_k_smooths_more() {
    let size = Size2us::new(96, 96);
    let px = noisy(size, 0.5, 0.04, 7);
    let mut img2 = gray(size, px.clone());
    let mut img5 = gray(size, px);
    Denoise {
        k: 2.0,
        ..Default::default()
    }
    .apply(&mut img2)
    .unwrap();
    Denoise {
        k: 5.0,
        ..Default::default()
    }
    .apply(&mut img5)
    .unwrap();
    let s2 = pixel_stats(img2.channel(0)).std as f32;
    let s5 = pixel_stats(img5.channel(0)).std as f32;
    assert!(
        s5 < s2,
        "higher k thresholds more, leaving less noise: s5 {s5} vs s2 {s2}"
    );
}

#[test]
fn strength_zero_is_identity_and_blends_between() {
    let size = Size2us::new(64, 64);
    let px = noisy(size, 0.5, 0.05, 3);

    // strength 0 removes nothing — bit-for-bit identity.
    let mut img0 = gray(size, px.clone());
    Denoise {
        strength: 0.0,
        ..Default::default()
    }
    .apply(&mut img0)
    .unwrap();
    assert_eq!(
        img0.channel(0).to_vec(),
        px,
        "strength 0 leaves the image untouched"
    );

    // The removed noise depends on the input alone, so the output is affine in the strength: half
    // strength is the midpoint of none and full, to the rounding of each scale's subtraction —
    // two per scale, over at most 6 scales, of values ≤ 0.8: 16ε of 0.8.
    let mut half = gray(size, px.clone());
    let mut full = gray(size, px.clone());
    Denoise {
        strength: 0.5,
        ..Default::default()
    }
    .apply(&mut half)
    .unwrap();
    Denoise {
        strength: 1.0,
        ..Default::default()
    }
    .apply(&mut full)
    .unwrap();
    let bound = 16.0 * f32::EPSILON * 0.8;
    for ((&h, &f), &p) in half
        .channel(0)
        .pixels()
        .iter()
        .zip(full.channel(0).pixels())
        .zip(&px)
    {
        assert!(
            (h - f32::midpoint(p, f)).abs() <= bound,
            "{h} between {p} and {f}"
        );
    }
}

#[test]
fn hard_and_soft_thresholds_differ() {
    let size = Size2us::new(64, 64);
    let px = noisy(size, 0.5, 0.05, 55);
    let mut hard = gray(size, px.clone());
    let mut soft = gray(size, px);
    Denoise {
        threshold: Threshold::Hard,
        ..Default::default()
    }
    .apply(&mut hard)
    .unwrap();
    Denoise {
        threshold: Threshold::Soft,
        ..Default::default()
    }
    .apply(&mut soft)
    .unwrap();
    let hv = hard.channel(0).to_vec();
    let sv = soft.channel(0).to_vec();
    assert!(hv != sv, "hard and soft produce different results");
    // Soft additionally shrinks the kept coefficients, so it is at least as smooth.
    assert!(
        (pixel_stats(&sv).std as f32) <= (pixel_stats(&hv).std as f32) + 1e-6,
        "soft no rougher than hard: soft {} hard {}",
        (pixel_stats(&sv).std as f32),
        (pixel_stats(&hv).std as f32)
    );
}

#[test]
fn denoise_preserves_bright_feature() {
    // A bright 8x8 block on a faintly-noisy background: hard thresholding keeps its large
    // coefficients, so the block stays bright while the flat background is smoothed.
    let size = Size2us::new(64, 64);
    let mut px = noisy(size, 0.1, 0.02, 808);
    for yy in 28..36 {
        for xx in 28..36 {
            px[yy * size.width + xx] = 0.9;
        }
    }
    let mut img = gray(size, px);
    Denoise::default().apply(&mut img).unwrap();
    let out = img.channel(0).to_vec();

    // 4x4 interior of the block stays near 0.9.
    let interior: Vec<f32> = (30..34)
        .flat_map(|yy| (30..34).map(move |xx| (yy, xx)))
        .map(|(yy, xx)| out[yy * size.width + xx])
        .collect();
    assert!(
        (pixel_stats(&interior).mean as f32) > 0.8,
        "bright feature preserved: interior mean {}",
        (pixel_stats(&interior).mean as f32)
    );
    // A flat corner far from the block is smoothed below the input noise floor.
    let corner: Vec<f32> = (0..10)
        .flat_map(|yy| (0..10).map(move |xx| (yy, xx)))
        .map(|(yy, xx)| out[yy * size.width + xx])
        .collect();
    assert!(
        (pixel_stats(&corner).std as f32) < 0.02,
        "background corner smoothed: {}",
        (pixel_stats(&corner).std as f32)
    );
}

/// Colour is denoised channel by channel, through one shared scratch: each channel comes out
/// exactly as that plane alone would, nothing carried over from the one before.
#[test]
fn denoise_is_per_channel_on_rgb() {
    let size = Size2us::new(48, 48);
    let planes = [
        noisy(size, 0.5, 0.03, 2024),
        noisy(size, 0.5, 0.05, 4048),
        noisy(size, 0.5, 0.04, 6072),
    ];
    let mut img = rgb(
        size,
        planes[0].clone(),
        planes[1].clone(),
        planes[2].clone(),
    );
    Denoise::default().apply(&mut img).unwrap();
    for (channel, plane) in planes.into_iter().enumerate() {
        let mut alone = gray(size, plane);
        Denoise::default().apply(&mut alone).unwrap();
        assert_eq!(img.channel(channel).pixels(), alone.channel(0).pixels());
    }
}

#[test]
fn denoise_handles_images_smaller_than_the_kernel() {
    // Scale count clamps to the dimensions — these must not panic.
    let mut tiny = gray(
        Size2us::new(3, 3),
        vec![0.1, 0.2, 0.3, 0.4, 0.5, 0.6, 0.7, 0.8, 0.9],
    );
    Denoise::default().apply(&mut tiny).unwrap();
    let mut one = gray(Size2us::new(1, 1), vec![0.42]);
    Denoise::default().apply(&mut one).unwrap();
    assert_eq!(
        one.channel(0).pixels(),
        &[0.42],
        "1x1 has no detail to remove"
    );
}
