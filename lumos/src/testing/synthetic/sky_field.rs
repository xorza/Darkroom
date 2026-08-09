//! A flat sky with Gaussian stars drawn straight onto it — the fixture most star-detection tests
//! start from.
//!
//! Three subsystems had their own copy of this (deblend, centroid and cosmic-ray stage tests),
//! differing only in sky level, noise sigma and whether they clamped.
//!
//! This is the *direct* render: no camera, no exposure, no sensor chain. Reach for
//! [`fixtures::star_field`](crate::testing::synthetic::fixtures::star_field) instead when the test
//! wants the full forward model and a `FrameTruth` to grade against; reach for this one when it
//! just needs pixels with stars at positions it chose.

use crate::math::size2us::Size2us;
use crate::math::vec2us::Vec2us;
use crate::testing::TestRng;
use crate::testing::synthetic::star_profiles::{StarProfile, SyntheticStar};
use glam::Vec2;
use imaginarium::Buffer2;

/// The sky a field is drawn on. Named fields because the three values are all small floats and
/// positionally interchangeable at a call site.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Sky {
    /// Flat background level.
    pub(crate) level: f32,
    /// Sigma of the Gaussian noise added per pixel; `0.0` for a noiseless sky.
    pub(crate) noise: f32,
    /// Clamp to `[0, 1]` after adding noise. Leave off when the test injects values above the
    /// ceiling afterwards — cosmic rays, for one.
    pub(crate) clamp: bool,
}

/// A rendered field and where its stars are.
#[derive(Debug)]
pub(crate) struct SkyField {
    pub(crate) pixels: Buffer2<f32>,
    /// Star centres rounded to the nearest pixel, for tests asserting a core survived.
    pub(crate) centers: Vec<Vec2us>,
}

impl SkyField {
    /// Draw `stars`, each `(centre, peak above the sky)`, as round Gaussians of width `sigma`.
    pub(crate) fn render(
        size: Size2us,
        sky: Sky,
        sigma: f32,
        stars: &[(Vec2, f32)],
        seed: u64,
    ) -> Self {
        let mut pixels = Buffer2::new_filled(size.width, size.height, sky.level);
        for &(center, peak) in stars {
            SyntheticStar::new(center, peak, StarProfile::Gaussian { sigma }).add_to(&mut pixels);
        }
        if sky.noise > 0.0 {
            let mut rng = TestRng::new(seed);
            for p in pixels.iter_mut() {
                *p += rng.next_gaussian_f32() * sky.noise;
            }
        }
        if sky.clamp {
            for p in pixels.iter_mut() {
                *p = p.clamp(0.0, 1.0);
            }
        }
        Self {
            pixels,
            centers: stars
                .iter()
                .map(|&(c, _)| Vec2us::new(c.x.round() as usize, c.y.round() as usize))
                .collect(),
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::math::size2us::Size2us;
    use crate::math::vec2us::Vec2us;
    use crate::testing::synthetic::sky_field::{Sky, SkyField};
    use glam::Vec2;

    const FLAT: Sky = Sky {
        level: 0.1,
        noise: 0.0,
        clamp: false,
    };

    #[test]
    fn stars_sit_on_the_sky_at_their_centres() {
        let field = SkyField::render(
            Size2us::new(32, 32),
            FLAT,
            1.5,
            &[(Vec2::new(8.0, 20.0), 0.5)],
            1,
        );
        // Peak is exactly the sky plus the star's amplitude, and it is where we put it.
        assert_eq!(field.pixels[(8, 20)], 0.6);
        assert_eq!(field.centers, vec![Vec2us::new(8, 20)]);
        // Far from the star, only sky.
        assert_eq!(field.pixels[(31, 0)], 0.1);
    }

    #[test]
    fn centres_round_to_the_nearest_pixel() {
        let field = SkyField::render(
            Size2us::new(16, 16),
            FLAT,
            1.0,
            &[(Vec2::new(4.4, 7.6), 0.3)],
            1,
        );
        assert_eq!(field.centers, vec![Vec2us::new(4, 8)]);
    }

    #[test]
    fn noise_is_deterministic_and_clamping_is_opt_in() {
        let noisy = Sky {
            level: 0.1,
            noise: 0.05,
            clamp: false,
        };
        let spec = |sky| SkyField::render(Size2us::new(24, 24), sky, 1.5, &[], 7);

        // Same seed, same field.
        assert_eq!(spec(noisy).pixels.pixels(), spec(noisy).pixels.pixels());
        // Noise moves it off the flat sky.
        assert!(spec(noisy).pixels.pixels().iter().any(|&v| v != 0.1));

        // A star brighter than the ceiling survives unclamped and is cut when clamping is on.
        let bright = [(Vec2::splat(12.0), 2.0)];
        let unclamped = SkyField::render(Size2us::new(24, 24), FLAT, 1.5, &bright, 7);
        let clamped = SkyField::render(
            Size2us::new(24, 24),
            Sky {
                clamp: true,
                ..FLAT
            },
            1.5,
            &bright,
            7,
        );
        assert!(unclamped.pixels[(12, 12)] > 1.0);
        assert_eq!(clamped.pixels[(12, 12)], 1.0);
    }
}
