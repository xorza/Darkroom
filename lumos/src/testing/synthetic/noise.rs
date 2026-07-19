//! Physical sensor-noise model, expressed in lumos' normalized full-well units.
//!
//! lumos pixels are normalized flux in `[0, 1]` where `1.0` == sensor full well. Real
//! sensors accumulate discrete photo-electrons, so the noise is Poisson shot noise on the
//! collected charge plus Gaussian read noise — not the constant-σ Gaussian the older
//! generators used. Modeling it physically (and converting back to normalized units) is
//! what makes a source's SNR mean what the detector's thresholds assume.
//!
//! Conversion: a normalized value `v` corresponds to `v * full_well_e` electrons.

use crate::testing::TestRng;

/// Draw a Poisson sample with mean `lambda` using `rng`.
///
/// Knuth's multiplicative method for `lambda < 30`; a Gaussian approximation
/// `N(λ, λ)` (clamped ≥ 0, rounded) above that, where the two distributions agree to
/// well under a percent and Knuth's loop would run ~λ iterations per sample.
pub fn poisson(rng: &mut TestRng, lambda: f32) -> f32 {
    if lambda <= 0.0 {
        return 0.0;
    }
    if lambda < 30.0 {
        // Knuth: count uniform draws until their product drops below e^-λ.
        let threshold = (-lambda).exp();
        let mut k = 0u32;
        let mut product = 1.0f32;
        loop {
            k += 1;
            product *= rng.next_f32();
            if product <= threshold {
                break;
            }
        }
        (k - 1) as f32
    } else {
        (lambda + lambda.sqrt() * rng.next_gaussian_f32())
            .max(0.0)
            .round()
    }
}

/// Apply Poisson shot noise to a normalized signal in place.
///
/// Each pixel `v` is treated as `v * full_well_e` electrons, resampled from a Poisson
/// distribution, and converted back to normalized units. Mean is preserved in
/// expectation; variance per pixel is `v / full_well_e`.
pub fn apply_shot_noise(pixels: &mut [f32], full_well_e: f32, rng: &mut TestRng) {
    assert!(full_well_e > 0.0, "full_well_e must be positive");
    for p in pixels.iter_mut() {
        let electrons = (*p * full_well_e).max(0.0);
        *p = poisson(rng, electrons) / full_well_e;
    }
}

/// Add Gaussian read noise (σ given in electrons) to a normalized signal in place.
///
/// The normalized standard deviation is `read_noise_e / full_well_e`.
pub fn add_read_noise(pixels: &mut [f32], read_noise_e: f32, full_well_e: f32, rng: &mut TestRng) {
    assert!(full_well_e > 0.0, "full_well_e must be positive");
    if read_noise_e <= 0.0 {
        return;
    }
    let sigma = read_noise_e / full_well_e;
    for p in pixels.iter_mut() {
        *p += rng.next_gaussian_f32() * sigma;
    }
}

/// Add a Poisson-distributed dark-current pedestal of mean `dark_current_e_per_s *
/// exposure_s` electrons per pixel, in normalized units, in place.
pub fn add_dark_current(
    pixels: &mut [f32],
    dark_current_e_per_s: f32,
    exposure_s: f32,
    full_well_e: f32,
    rng: &mut TestRng,
) {
    assert!(full_well_e > 0.0, "full_well_e must be positive");
    let lambda = dark_current_e_per_s * exposure_s;
    if lambda <= 0.0 {
        return;
    }
    for p in pixels.iter_mut() {
        *p += poisson(rng, lambda) / full_well_e;
    }
}

#[cfg(test)]
mod tests {
    use crate::testing::synthetic::noise::*;

    fn mean_var(samples: &[f32]) -> (f64, f64) {
        let n = samples.len() as f64;
        let mean = samples.iter().map(|&s| s as f64).sum::<f64>() / n;
        let var = samples
            .iter()
            .map(|&s| (s as f64 - mean).powi(2))
            .sum::<f64>()
            / n;
        (mean, var)
    }

    #[test]
    fn poisson_mean_and_variance_small_lambda() {
        // Knuth branch. For Poisson(λ), E=λ and Var=λ. With N=200_000 draws the standard
        // error of the mean is sqrt(10/200_000) ≈ 0.0071, so ±0.1 is ~14σ — a safe bound.
        let mut rng = TestRng::new(1);
        let samples: Vec<f32> = (0..200_000).map(|_| poisson(&mut rng, 10.0)).collect();
        let (mean, var) = mean_var(&samples);
        assert!((mean - 10.0).abs() < 0.1, "mean {mean}");
        assert!((var - 10.0).abs() < 0.6, "var {var}");
        // Poisson is integer-valued.
        assert!(samples.iter().all(|&s| s == s.round()));
    }

    #[test]
    fn poisson_normal_branch_large_lambda() {
        let mut rng = TestRng::new(2);
        let samples: Vec<f32> = (0..200_000).map(|_| poisson(&mut rng, 1000.0)).collect();
        let (mean, var) = mean_var(&samples);
        assert!((mean - 1000.0).abs() < 5.0, "mean {mean}");
        assert!((var - 1000.0).abs() < 60.0, "var {var}");
    }

    #[test]
    fn poisson_zero_lambda_is_zero() {
        let mut rng = TestRng::new(3);
        for _ in 0..10 {
            assert_eq!(poisson(&mut rng, 0.0), 0.0);
        }
    }

    #[test]
    fn shot_noise_preserves_mean_and_sets_variance() {
        // Uniform 0.5 signal at 10_000 e full well: per-pixel variance = 0.5/10_000 = 5e-5,
        // std ≈ 0.00707. Mean must stay ≈ 0.5.
        let mut rng = TestRng::new(4);
        let mut pixels = vec![0.5f32; 100_000];
        apply_shot_noise(&mut pixels, 10_000.0, &mut rng);
        let (mean, var) = mean_var(&pixels);
        assert!((mean - 0.5).abs() < 1e-3, "mean {mean}");
        assert!((var - 5e-5).abs() < 1e-5, "var {var}");
    }

    #[test]
    fn read_noise_has_expected_sigma() {
        // read_noise 5 e at 10_000 e full well → σ_norm = 5e-4, mean 0.
        let mut rng = TestRng::new(5);
        let mut pixels = vec![0.0f32; 100_000];
        add_read_noise(&mut pixels, 5.0, 10_000.0, &mut rng);
        let (mean, var) = mean_var(&pixels);
        assert!(mean.abs() < 1e-5, "mean {mean}");
        assert!((var.sqrt() - 5e-4).abs() < 5e-5, "std {}", var.sqrt());
    }

    #[test]
    fn dark_current_adds_expected_pedestal() {
        // 0.1 e/s for 100 s = 10 e mean → normalized 10/10_000 = 1e-3.
        let mut rng = TestRng::new(6);
        let mut pixels = vec![0.0f32; 100_000];
        add_dark_current(&mut pixels, 0.1, 100.0, 10_000.0, &mut rng);
        let (mean, _) = mean_var(&pixels);
        assert!((mean - 1e-3).abs() < 5e-5, "mean {mean}");
    }

    #[test]
    fn read_noise_zero_is_noop() {
        let mut rng = TestRng::new(7);
        let mut pixels = vec![0.3f32; 100];
        add_read_noise(&mut pixels, 0.0, 10_000.0, &mut rng);
        assert!(pixels.iter().all(|&p| p == 0.3));
    }

    #[test]
    fn poisson_sub_unit_lambda_is_small_integers() {
        // Dark current's actual regime (λ≈0.05): mean tracks λ; draws are 0/1/(rare 2).
        let mut rng = TestRng::new(11);
        let samples: Vec<f32> = (0..200_000).map(|_| poisson(&mut rng, 0.05)).collect();
        let (mean, _) = mean_var(&samples);
        assert!((mean - 0.05).abs() < 0.005, "mean {mean}");
        assert!(
            samples
                .iter()
                .all(|&s| s == s.round() && (0.0..=3.0).contains(&s)),
            "sub-unit Poisson should be small integers"
        );
    }

    #[test]
    fn poisson_branch_boundary_at_30() {
        // λ=30 takes the Gaussian branch; mean and variance must still equal λ.
        let mut rng = TestRng::new(12);
        let samples: Vec<f32> = (0..200_000).map(|_| poisson(&mut rng, 30.0)).collect();
        let (mean, var) = mean_var(&samples);
        assert!((mean - 30.0).abs() < 0.3, "mean {mean}");
        assert!((var - 30.0).abs() < 2.5, "var {var}");
    }

    #[test]
    fn shot_noise_variance_scales_with_signal() {
        // Var = v/full_well, so a 4× brighter signal has ~4× the per-pixel variance at one well.
        let mut rng = TestRng::new(13);
        let well = 5_000.0;
        let mut dim = vec![0.2f32; 200_000];
        let mut bright = vec![0.8f32; 200_000];
        apply_shot_noise(&mut dim, well, &mut rng);
        apply_shot_noise(&mut bright, well, &mut rng);
        let (_, var_dim) = mean_var(&dim);
        let (_, var_bright) = mean_var(&bright);
        let ratio = var_bright / var_dim;
        assert!(
            (ratio - 4.0).abs() < 0.4,
            "shot-noise variance must scale with signal: ratio {ratio:.2}"
        );
    }
}
