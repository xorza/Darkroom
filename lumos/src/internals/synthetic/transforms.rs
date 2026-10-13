//! Random star fields for registration tests, and the noise, false detections and positions they
//! are disturbed by.

use crate::internals::test_rng::TestRng;
use crate::star_detection::star::Star;
use glam::DVec2;

/// Uniformly random stars over a square field, deterministic in `seed`.
#[derive(Debug, Clone, Copy)]
pub(crate) struct StarScatter {
    pub(crate) count: usize,
    /// The field's width and height, in pixels.
    pub(crate) side: f64,
    pub(crate) seed: u64,
}

impl StarScatter {
    /// The star positions, 50 px clear of every edge.
    pub(crate) fn positions(self) -> Vec<DVec2> {
        self.points(50.0).collect()
    }

    /// The [`positions`](Self::positions) as stars of a uniform `fwhm`.
    pub(crate) fn stars(self, fwhm: f32) -> Vec<Star> {
        positions_to_stars(&self.positions(), fwhm)
    }

    /// Faint stars 10 px clear of every edge, as false detections.
    pub(crate) fn spurious(self, fwhm: f32) -> Vec<Star> {
        // Faint by construction: a spurious detection has to look like one to any grader that
        // later learns to weigh SNR.
        self.points(10.0)
            .enumerate()
            .map(|(i, pos)| {
                Star::at(pos)
                    .with_flux(100.0 - i as f32)
                    .with_fwhm(fwhm)
                    .with_snr(10.0)
                    .with_peak(0.1)
            })
            .collect()
    }

    fn points(self, margin: f64) -> impl Iterator<Item = DVec2> {
        let mut rng = TestRng::new(self.seed);
        let span = self.side - 2.0 * margin;
        (0..self.count).map(move |_| {
            let x = margin + rng.next_f64() * span;
            let y = margin + rng.next_f64() * span;
            DVec2::new(x, y)
        })
    }
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

#[cfg(test)]
mod tests {
    use crate::internals::synthetic::transforms::*;

    #[test]
    fn random_positions_stay_inside_the_margin_and_repeat_for_a_seed() {
        let scatter = StarScatter {
            count: 100,
            side: 1000.0,
            seed: 12345,
        };
        let stars = scatter.positions();
        assert_eq!(stars.len(), 100);

        // Check all stars are within bounds (with margin)
        for p in &stars {
            assert!(p.x >= 50.0 && p.x <= 950.0);
            assert!(p.y >= 50.0 && p.y <= 950.0);
        }

        // Check reproducibility
        assert_eq!(stars, scatter.positions());
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
