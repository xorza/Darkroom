use super::{Denoise, Threshold};
use crate::image_ops::op::OpError;
use crate::image_ops::test_support::channel_plane as channel;
use imaginarium::{Buffer2, ColorFormat, DeinterleavedImageData, Image, ImageDesc};

/// Deterministic xorshift64 + Box-Muller Gaussian, so noise tests are reproducible without a dep.
struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Self {
        Rng(seed | 1)
    }

    fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }

    /// Uniform in `(0, 1]`.
    fn next_unit(&mut self) -> f32 {
        ((self.next_u64() >> 40) as f32 + 1.0) / (1u64 << 24) as f32
    }

    /// Standard-normal sample.
    fn next_gaussian(&mut self) -> f32 {
        let u1 = self.next_unit();
        let u2 = self.next_unit();
        (-2.0 * u1.ln()).sqrt() * (std::f32::consts::TAU * u2).cos()
    }
}

fn gray(width: usize, height: usize, px: Vec<f32>) -> Image {
    Image::from(&DeinterleavedImageData::from_channels([Buffer2::new(
        width, height, px,
    )]))
}

fn rgb(width: usize, height: usize, r: Vec<f32>, g: Vec<f32>, b: Vec<f32>) -> Image {
    Image::from(&DeinterleavedImageData::from_channels([
        Buffer2::new(width, height, r),
        Buffer2::new(width, height, g),
        Buffer2::new(width, height, b),
    ]))
}

/// Channel `c` of an image as a buffer (for assertions).
fn mean(data: &[f32]) -> f32 {
    data.iter().sum::<f32>() / data.len() as f32
}

fn std_dev(data: &[f32]) -> f32 {
    let m = mean(data);
    (data.iter().map(|&v| (v - m) * (v - m)).sum::<f32>() / data.len() as f32).sqrt()
}

fn noisy(width: usize, height: usize, bg: f32, sigma: f32, seed: u64) -> Vec<f32> {
    let mut rng = Rng::new(seed);
    (0..width * height)
        .map(|_| bg + rng.next_gaussian() * sigma)
        .collect()
}

#[test]
fn threshold_apply_hand_computed() {
    // Hard: keep |w| >= t, else 0.
    assert_eq!(Threshold::Hard.apply(0.3, 0.2), 0.3);
    assert_eq!(Threshold::Hard.apply(-0.3, 0.2), -0.3);
    assert_eq!(Threshold::Hard.apply(0.1, 0.2), 0.0);
    assert_eq!(Threshold::Hard.apply(0.2, 0.2), 0.2); // boundary is kept (>=)
    // Soft: sign(w) * max(|w| - t, 0).
    assert!((Threshold::Soft.apply(0.3, 0.2) - 0.1).abs() < 1e-6);
    assert!((Threshold::Soft.apply(-0.3, 0.2) + 0.1).abs() < 1e-6);
    assert_eq!(Threshold::Soft.apply(0.1, 0.2), 0.0);
}

#[test]
fn denoise_reduces_white_noise_and_preserves_mean() {
    let (w, h) = (128, 128);
    let (bg, sigma) = (0.5, 0.05);
    let px = noisy(w, h, bg, sigma, 12345);
    let in_std = std_dev(&px);

    let mut img = gray(w, h, px);
    Denoise::default().apply(&mut img).unwrap();
    let out = channel(&img, 0).to_vec();

    let out_std = std_dev(&out);
    assert!(
        out_std < 0.6 * in_std,
        "white noise reduced: out_std {out_std} vs in_std {in_std}"
    );
    assert!(
        (mean(&out) - bg).abs() < 0.01,
        "DC preserved near {bg}: {}",
        mean(&out)
    );
}

#[test]
fn higher_k_smooths_more() {
    let (w, h) = (96, 96);
    let px = noisy(w, h, 0.5, 0.04, 7);
    let mut img2 = gray(w, h, px.clone());
    let mut img5 = gray(w, h, px);
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
    let s2 = std_dev(&channel(&img2, 0));
    let s5 = std_dev(&channel(&img5, 0));
    assert!(
        s5 < s2,
        "higher k thresholds more, leaving less noise: s5 {s5} vs s2 {s2}"
    );
}

#[test]
fn strength_zero_is_identity_and_blends_between() {
    let (w, h) = (64, 64);
    let px = noisy(w, h, 0.5, 0.05, 3);

    // strength 0 removes nothing — bit-for-bit identity.
    let mut img0 = gray(w, h, px.clone());
    Denoise {
        strength: 0.0,
        ..Default::default()
    }
    .apply(&mut img0)
    .unwrap();
    assert_eq!(
        channel(&img0, 0).to_vec(),
        px,
        "strength 0 leaves the image untouched"
    );

    // Partial strength sits strictly between no-op and full denoise.
    let mut half = gray(w, h, px.clone());
    let mut full = gray(w, h, px.clone());
    Denoise {
        strength: 0.5,
        ..Default::default()
    }
    .apply(&mut half)
    .unwrap();
    Denoise::default().apply(&mut full).unwrap();
    let in_std = std_dev(&px);
    let half_std = std_dev(&channel(&half, 0));
    let full_std = std_dev(&channel(&full, 0));
    assert!(
        full_std < half_std && half_std < in_std,
        "blend ordering: full {full_std} < half {half_std} < in {in_std}"
    );
}

#[test]
fn hard_and_soft_thresholds_differ() {
    let (w, h) = (64, 64);
    let px = noisy(w, h, 0.5, 0.05, 55);
    let mut hard = gray(w, h, px.clone());
    let mut soft = gray(w, h, px);
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
    let hv = channel(&hard, 0).to_vec();
    let sv = channel(&soft, 0).to_vec();
    assert!(hv != sv, "hard and soft produce different results");
    // Soft additionally shrinks the kept coefficients, so it is at least as smooth.
    assert!(
        std_dev(&sv) <= std_dev(&hv) + 1e-6,
        "soft no rougher than hard: soft {} hard {}",
        std_dev(&sv),
        std_dev(&hv)
    );
}

#[test]
fn denoise_preserves_bright_feature() {
    // A bright 8x8 block on a faintly-noisy background: hard thresholding keeps its large
    // coefficients, so the block stays bright while the flat background is smoothed.
    let (w, h) = (64, 64);
    let mut px = noisy(w, h, 0.1, 0.02, 808);
    for yy in 28..36 {
        for xx in 28..36 {
            px[yy * w + xx] = 0.9;
        }
    }
    let mut img = gray(w, h, px);
    Denoise::default().apply(&mut img).unwrap();
    let out = channel(&img, 0).to_vec();

    // 4x4 interior of the block stays near 0.9.
    let interior: Vec<f32> = (30..34)
        .flat_map(|yy| (30..34).map(move |xx| (yy, xx)))
        .map(|(yy, xx)| out[yy * w + xx])
        .collect();
    assert!(
        mean(&interior) > 0.8,
        "bright feature preserved: interior mean {}",
        mean(&interior)
    );
    // A flat corner far from the block is smoothed below the input noise floor.
    let corner: Vec<f32> = (0..10)
        .flat_map(|yy| (0..10).map(move |xx| (yy, xx)))
        .map(|(yy, xx)| out[yy * w + xx])
        .collect();
    assert!(
        std_dev(&corner) < 0.02,
        "background corner smoothed: {}",
        std_dev(&corner)
    );
}

#[test]
fn denoise_is_per_channel_on_rgb() {
    let (w, h) = (48, 48);
    let r = noisy(w, h, 0.5, 0.03, 2024);
    let g = noisy(w, h, 0.5, 0.05, 4048);
    let b = noisy(w, h, 0.5, 0.04, 6072);
    let in_std = [std_dev(&r), std_dev(&g), std_dev(&b)];
    let mut img = rgb(w, h, r, g, b);
    Denoise::default().apply(&mut img).unwrap();
    for (c, &expected) in in_std.iter().enumerate() {
        let out_std = std_dev(&channel(&img, c));
        assert!(
            out_std < expected,
            "channel {c} denoised: {out_std} < {expected}"
        );
    }
}

#[test]
fn denoise_handles_images_smaller_than_the_kernel() {
    // Scale count clamps to the dimensions — these must not panic.
    let mut tiny = gray(3, 3, vec![0.1, 0.2, 0.3, 0.4, 0.5, 0.6, 0.7, 0.8, 0.9]);
    Denoise::default().apply(&mut tiny).unwrap();
    let mut one = gray(1, 1, vec![0.42]);
    Denoise::default().apply(&mut one).unwrap();
    assert!(
        (channel(&one, 0).to_vec()[0] - 0.42).abs() < 1e-6,
        "1x1 has no detail to remove"
    );
}

#[test]
fn validate_rejects_out_of_range_strength() {
    let mut img = gray(4, 4, vec![0.0; 16]);
    let err = Denoise {
        strength: 1.5,
        ..Default::default()
    }
    .apply(&mut img)
    .unwrap_err();
    assert!(
        matches!(&err, OpError::InvalidConfig(m) if m.contains("strength must be in")),
        "expected an InvalidConfig strength error, got {err:?}"
    );
}

#[test]
fn validate_rejects_nonpositive_k() {
    let mut img = gray(4, 4, vec![0.0; 16]);
    let err = Denoise {
        k: 0.0,
        ..Default::default()
    }
    .apply(&mut img)
    .unwrap_err();
    assert!(
        matches!(&err, OpError::InvalidConfig(m) if m.contains("k must")),
        "expected an InvalidConfig k error, got {err:?}"
    );
}

#[test]
fn rejects_non_f32_master() {
    // An 8-bit image is not a linear f32 master, so the op is rejected before running.
    let mut img = Image::new_with_data(
        ImageDesc::new(4, 4, ColorFormat::RGB_U8),
        vec![0u8; 4 * 4 * 3],
    )
    .unwrap();
    let err = Denoise::default().apply(&mut img).unwrap_err();
    assert!(
        matches!(err, OpError::UnsupportedFormat(ColorFormat::RGB_U8)),
        "expected UnsupportedFormat(RGB_U8), got {err:?}"
    );
}
