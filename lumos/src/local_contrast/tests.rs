use super::{LocalContrastConfig, build_tile_luts, enhance_local_contrast_planar};
use crate::io::astro_image::{AstroImage, ImageDimensions};
use common::Vec2us;
use imaginarium::Buffer2;

fn gray(width: usize, height: usize, px: Vec<f32>) -> AstroImage {
    AstroImage::from_planar_channels(ImageDimensions::new(Vec2us::new(width, height), 1), [px])
}

fn rgb(width: usize, height: usize, r: Vec<f32>, g: Vec<f32>, b: Vec<f32>) -> AstroImage {
    AstroImage::from_planar_channels(
        ImageDimensions::new(Vec2us::new(width, height), 3),
        [r, g, b],
    )
}

fn mean(d: &[f32]) -> f32 {
    d.iter().sum::<f32>() / d.len() as f32
}

fn std_dev(d: &[f32]) -> f32 {
    let m = mean(d);
    (d.iter().map(|&v| (v - m) * (v - m)).sum::<f32>() / d.len() as f32).sqrt()
}

/// A low-contrast horizontal gradient (intensity in `[0.45, 0.55]`).
fn low_contrast(width: usize, height: usize) -> Vec<f32> {
    (0..width * height)
        .map(|i| 0.45 + 0.1 * ((i % width) as f32 / (width - 1) as f32))
        .collect()
}

#[test]
fn clahe_strength_zero_is_identity() {
    let px = low_contrast(64, 64);
    let mut img = gray(64, 64, px.clone());
    enhance_local_contrast_planar(
        &mut img,
        LocalContrastConfig {
            strength: 0.0,
            ..Default::default()
        },
    );
    assert_eq!(
        img.channel(0).to_vec(),
        px,
        "strength 0 leaves the image untouched"
    );
}

#[test]
fn clahe_output_stays_in_range() {
    let px: Vec<f32> = (0..96 * 96)
        .map(|i| ((i as f32 * 0.013).sin() * 0.5 + 0.5).clamp(0.0, 1.0))
        .collect();
    let mut img = gray(96, 96, px);
    enhance_local_contrast_planar(&mut img, LocalContrastConfig::default());
    for &v in &img.channel(0).to_vec() {
        assert!((0.0..=1.0).contains(&v), "output in [0,1]: {v}");
    }
}

#[test]
fn clahe_flat_region_not_blown_up() {
    // Contrast-limited: a flat field must stay put, not get stretched to full range.
    let mut img = gray(64, 64, vec![0.5; 64 * 64]);
    enhance_local_contrast_planar(&mut img, LocalContrastConfig::default());
    let out = img.channel(0).to_vec();
    assert!(
        out.iter().all(|&v| (v - 0.5).abs() < 0.05),
        "flat 0.5 stays ~0.5 (mean {})",
        mean(&out)
    );
}

#[test]
fn clahe_increases_low_contrast() {
    // A low-contrast gradient gets its local contrast expanded → higher spread.
    let px = low_contrast(64, 64);
    let in_std = std_dev(&px);
    let mut img = gray(64, 64, px);
    enhance_local_contrast_planar(
        &mut img,
        LocalContrastConfig {
            tiles: 4,
            clip_limit: 4.0,
            strength: 1.0,
        },
    );
    let out_std = std_dev(img.channel(0));
    assert!(
        out_std > in_std,
        "local contrast expanded: {out_std} > {in_std}"
    );
}

#[test]
fn clahe_tile_mappings_are_monotonic() {
    let px: Vec<f32> = (0..80 * 80)
        .map(|i| ((i % 80) as f32 / 79.0 + (i / 80) as f32 / 79.0) * 0.5)
        .collect();
    let intensity = Buffer2::new(80, 80, px);
    let luts = build_tile_luts(&intensity, 4, 2.0);
    for lut in &luts {
        for w in lut.windows(2) {
            assert!(
                w[1] >= w[0] - 1e-6,
                "LUT must be non-decreasing: {} -> {}",
                w[0],
                w[1]
            );
        }
    }
}

#[test]
fn clahe_is_color_preserving() {
    // A 2:1:1 R:G:B field keeps its ratio (hue) through the intensity-based mapping.
    let (w, h) = (64, 64);
    let i: Vec<f32> = low_contrast(w, h); // use as the green/blue level
    let r: Vec<f32> = i.iter().map(|&v| (2.0 * v).min(1.0)).collect();
    let mut img = rgb(w, h, r, i.clone(), i.clone());
    enhance_local_contrast_planar(&mut img, LocalContrastConfig::default());
    let (ro, go, bo) = (
        img.channel(0).to_vec(),
        img.channel(1).to_vec(),
        img.channel(2).to_vec(),
    );
    assert_eq!(go, bo, "G and B stay equal");
    // Where red isn't clamped at 1, the 2:1 ratio is preserved.
    for k in 0..ro.len() {
        if ro[k] < 0.999 && go[k] > 1e-3 {
            assert!(
                (ro[k] / go[k] - 2.0).abs() < 0.05,
                "R:G ratio preserved at {k}: {} {}",
                ro[k],
                go[k]
            );
        }
    }
}
