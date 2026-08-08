use crate::io::image::image_dimensions::ImageDimensions;
use crate::io::image::linear::LinearImage;
use crate::math::size2us::Size2us;
use crate::stacking::star_detection::detector::stages::prepare::*;

/// Build a 16-pixel channel whose median is `center` and MAD is exactly `mad`
/// (8 pixels at `center - mad`, 8 at `center + mad`).
fn channel_with_mad(center: f32, mad: f32) -> Vec<f32> {
    let mut v = vec![center - mad; 8];
    v.extend(vec![center + mad; 8]);
    v
}

#[test]
fn test_prepare_uniform() {
    let dim = ImageDimensions::new((64, 64), 1);
    let data = vec![0.5f32; 64 * 64];
    let image = LinearImage::from_pixels(dim, data);

    let mut pool = DetectionResources::new(Size2us::new(64, 64));
    let result = prepare(&image, &mut pool);

    assert_eq!(result.width(), 64);
    assert_eq!(result.height(), 64);
    for &v in result.pixels() {
        assert!((v - 0.5).abs() < 1e-6);
    }
}

#[test]
fn test_prepare_with_star() {
    let width = 64;
    let height = 64;
    let mut data = vec![0.1f32; width * height];
    // Add bright pixel (simulating a star)
    data[32 * width + 32] = 0.9;

    let dim = ImageDimensions::new((width, height), 1);
    let image = LinearImage::from_pixels(dim, data);

    let mut pool = DetectionResources::new(Size2us::new(width, height));
    let result = prepare(&image, &mut pool);

    // Star pixel should be preserved (no CFA, no defects)
    assert!((result[(32, 32)] - 0.9).abs() < 1e-6);
}

#[test]
fn test_detection_weights_equal_noise() {
    // Identical per-channel MAD → equal inverse-variance weights (≈ 1/3 each).
    let dims = ImageDimensions::new((4, 4), 3);
    let ch = channel_with_mad(0.5, 0.02);
    let image = LinearImage::from_planar_channels(dims, vec![ch.clone(), ch.clone(), ch]);

    let mut scratch = [
        Buffer2::new_default(4, 4),
        Buffer2::new_default(4, 4),
        Buffer2::new_default(4, 4),
    ];
    let w = detection_channel_weights(&image, &mut scratch);

    for &wi in &w {
        assert!((wi - 1.0 / 3.0).abs() < 1e-4, "expected ~1/3, got {wi}");
    }
    assert!((w.iter().sum::<f32>() - 1.0).abs() < 1e-5);
}

#[test]
fn test_detection_weights_downweight_noisy_channel() {
    // R,G clean (MAD 0.02), B noisy (MAD 0.08). σ ∝ MAD, so w ∝ 1/MAD².
    // w_R / w_B = (0.08 / 0.02)² = 16.
    let dims = ImageDimensions::new((4, 4), 3);
    let clean = channel_with_mad(0.5, 0.02);
    let noisy = channel_with_mad(0.5, 0.08);
    let image = LinearImage::from_planar_channels(dims, vec![clean.clone(), clean, noisy]);

    let mut scratch = [
        Buffer2::new_default(4, 4),
        Buffer2::new_default(4, 4),
        Buffer2::new_default(4, 4),
    ];
    let w = detection_channel_weights(&image, &mut scratch);

    assert!((w[0] - w[1]).abs() < 1e-5, "R and G have equal noise");
    let ratio = w[0] / w[2];
    assert!(
        (ratio - 16.0).abs() < 0.05,
        "w_R/w_B should be ~16, got {ratio}"
    );
    assert!((w.iter().sum::<f32>() - 1.0).abs() < 1e-5);
}

#[test]
fn test_detection_weights_all_flat_falls_back_to_mean() {
    // Uniform channels have MAD 0 → degenerate; weights fall back to 1/3 each.
    let dims = ImageDimensions::new((4, 4), 3);
    let flat = vec![0.5f32; 16];
    let image = LinearImage::from_planar_channels(dims, vec![flat.clone(), flat.clone(), flat]);

    let mut scratch = [
        Buffer2::new_default(4, 4),
        Buffer2::new_default(4, 4),
        Buffer2::new_default(4, 4),
    ];
    let w = detection_channel_weights(&image, &mut scratch);

    assert_eq!(w, [1.0 / 3.0; 3]);
}

#[test]
fn test_prepare_rgb_equal_noise_is_mean() {
    // Distinct per-channel levels but identical spread → equal weights, so the
    // detection plane is the plain mean of the three channels.
    let dims = ImageDimensions::new((4, 4), 3);
    let r = channel_with_mad(0.30, 0.02);
    let g = channel_with_mad(0.50, 0.02);
    let b = channel_with_mad(0.70, 0.02);
    let image = LinearImage::from_planar_channels(dims, vec![r.clone(), g.clone(), b.clone()]);

    let mut pool = DetectionResources::new(Size2us::new(4, 4));
    let out = prepare(&image, &mut pool);

    for (i, &out_v) in out.pixels().iter().enumerate() {
        let expected = (r[i] + g[i] + b[i]) / 3.0;
        assert!(
            (out_v - expected).abs() < 1e-4,
            "pixel {i}: expected mean {expected}, got {out_v}"
        );
    }
}

#[test]
fn test_prepare_rgb_red_star_survives() {
    // A star bright only in R must remain prominent in the detection plane.
    // With equal-noise channels the weights are ~1/3, so the star peak lands at
    // ~1/3 of its R amplitude — far above Rec.709's 0.21× crush of red.
    let dims = ImageDimensions::new((4, 4), 3);
    let mut r = channel_with_mad(0.10, 0.01);
    r[5] = 0.90; // bright red star, off the symmetric background
    let g = channel_with_mad(0.10, 0.01);
    let b = channel_with_mad(0.10, 0.01);
    let image = LinearImage::from_planar_channels(dims, vec![r, g, b]);

    let mut pool = DetectionResources::new(Size2us::new(4, 4));
    let out = prepare(&image, &mut pool);

    // Background ~0.10; star pixel should be well above it (~0.10*2/3 + 0.90/3 ≈ 0.37).
    assert!(out[5] > 0.30, "red star should survive, got {}", out[5]);
}
