//! Physical sensor-noise model, expressed in lumos' normalized full-well units.
//!
//! lumos pixels are normalized flux in `[0, 1]` where `1.0` == sensor full well. Real
//! sensors accumulate discrete photo-electrons, so the noise is Poisson shot noise on the
//! collected charge plus Gaussian read noise. Modeling it physically (and converting back to
//! normalized units) is what makes a source's SNR mean what the detector's thresholds assume.
//! A fixture that wants one constant σ instead uses `patterns::add_gaussian_noise`.
//!
//! Conversion: a normalized value `v` corresponds to `v * full_well_e` electrons.

use crate::internals::test_rng::TestRng;

/// Draw a Poisson sample with mean `lambda` using `rng`.
///
/// Knuth's multiplicative method for `lambda < 30`; a Gaussian approximation
/// `N(λ, λ)` (clamped ≥ 0, rounded) above that, where the two distributions agree to
/// well under a percent and Knuth's loop would run ~λ iterations per sample.
fn poisson(rng: &mut TestRng, lambda: f32) -> f32 {
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
pub(crate) fn apply_shot_noise(pixels: &mut [f32], full_well_e: f32, rng: &mut TestRng) {
    assert!(full_well_e > 0.0, "full_well_e must be positive");
    for p in pixels.iter_mut() {
        let electrons = (*p * full_well_e).max(0.0);
        *p = poisson(rng, electrons) / full_well_e;
    }
}

/// Add Gaussian read noise (σ given in electrons) to a normalized signal in place.
///
/// The normalized standard deviation is `read_noise_e / full_well_e`.
pub(crate) fn add_read_noise(
    pixels: &mut [f32],
    read_noise_e: f32,
    full_well_e: f32,
    rng: &mut TestRng,
) {
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
pub(super) fn add_dark_current(
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
mod tests;
