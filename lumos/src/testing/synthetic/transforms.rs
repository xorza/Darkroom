//! Random star fields for registration tests, and the noise, false detections and positions they
//! are disturbed by.

use crate::stacking::star_detection::star::Star;
use crate::testing::test_rng::TestRng;
use glam::DVec2;

/// Generate random star positions within a bounded area (default 50-px margin).
///
/// Uses a deterministic LCG random number generator for reproducibility.
pub(crate) fn generate_random_positions(
    num_stars: usize,
    width: f64,
    height: f64,
    seed: u64,
) -> Vec<DVec2> {
    let margin = 50.0;
    let mut rng = TestRng::new(seed);
    let mut stars = Vec::with_capacity(num_stars);

    for _ in 0..num_stars {
        let x = margin + rng.next_f64() * (width - 2.0 * margin);
        let y = margin + rng.next_f64() * (height - 2.0 * margin);
        stars.push(DVec2::new(x, y));
    }

    stars
}

/// Convert positions to Star structs with uniform properties.
///
/// Creates Star structs suitable for registration testing with default properties.
/// The FWHM is set uniformly to allow `register()` to derive `max_sigma` correctly.
pub(crate) fn positions_to_stars(positions: &[DVec2], fwhm: f32) -> Vec<Star> {
    positions
        .iter()
        .enumerate()
        .map(|(i, &pos)| {
            Star::at(pos)
                .with_flux(10000.0 - i as f32 * 10.0)
                .with_fwhm(fwhm)
        })
        .collect()
}

/// Generate random star positions and convert to Star structs.
pub(crate) fn generate_random_stars(
    num_stars: usize,
    width: f64,
    height: f64,
    seed: u64,
    fwhm: f32,
) -> Vec<Star> {
    let positions = generate_random_positions(num_stars, width, height, seed);
    positions_to_stars(&positions, fwhm)
}

/// Add positional noise to Stars.
pub(crate) fn add_star_noise(stars: &[Star], noise_amplitude: f64, seed: u64) -> Vec<Star> {
    let mut rng = TestRng::new(seed);
    stars
        .iter()
        .map(|s| {
            let noise = DVec2::new(
                (rng.next_f64() * 2.0 - 1.0) * noise_amplitude,
                (rng.next_f64() * 2.0 - 1.0) * noise_amplitude,
            );
            s.with_pos(s.pos + noise)
        })
        .collect()
}

/// Add random spurious stars (simulate false detections).
pub(crate) fn add_spurious_star_list(
    stars: &[Star],
    count: usize,
    width: f64,
    height: f64,
    seed: u64,
    fwhm: f32,
) -> Vec<Star> {
    let margin = 10.0;
    let mut result = stars.to_vec();
    let mut rng = TestRng::new(seed);

    for i in 0..count {
        let x = margin + rng.next_f64() * (width - 2.0 * margin);
        let y = margin + rng.next_f64() * (height - 2.0 * margin);
        // Faint by construction: a spurious detection has to look like one to any grader that
        // later learns to weigh SNR.
        result.push(
            Star::at(DVec2::new(x, y))
                .with_flux(100.0 - i as f32)
                .with_fwhm(fwhm)
                .with_snr(10.0)
                .with_peak(0.1),
        );
    }

    result
}

#[cfg(test)]
mod tests {
    use crate::testing::synthetic::transforms::*;

    #[test]
    fn random_positions_stay_inside_the_margin_and_repeat_for_a_seed() {
        let stars = generate_random_positions(100, 1000.0, 1000.0, 12345);
        assert_eq!(stars.len(), 100);

        // Check all stars are within bounds (with margin)
        for p in &stars {
            assert!(p.x >= 50.0 && p.x <= 950.0);
            assert!(p.y >= 50.0 && p.y <= 950.0);
        }

        // Check reproducibility
        let stars2 = generate_random_positions(100, 1000.0, 1000.0, 12345);
        assert_eq!(stars, stars2);
    }

    #[test]
    fn star_noise_moves_positions_within_its_amplitude() {
        let stars = positions_to_stars(&vec![DVec2::new(500.0, 500.0); 100], 3.0);
        let noisy = add_star_noise(&stars, 1.0, 12345);

        let mut has_different = false;
        for (orig, noisy) in stars.iter().zip(noisy.iter()) {
            if (orig.pos.x - noisy.pos.x).abs() > 1e-10 || (orig.pos.y - noisy.pos.y).abs() > 1e-10
            {
                has_different = true;
            }
            // Noise stays within amplitude.
            assert!((orig.pos.x - noisy.pos.x).abs() <= 1.0);
            assert!((orig.pos.y - noisy.pos.y).abs() <= 1.0);
        }
        assert!(has_different);
    }
}
