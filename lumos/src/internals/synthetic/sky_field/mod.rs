//! A flat sky with Gaussian stars drawn straight onto it — the fixture most star-detection tests
//! start from.
//!
//! Three subsystems had their own copy of this (deblend, centroid and cosmic-ray stage tests),
//! differing only in sky level, noise sigma and whether they clamped.
//!
//! This is the *direct* render: no camera, no exposure, no sensor chain. Reach for
//! [`fixtures::star_field`](crate::internals::synthetic::fixtures::star_field) instead when the test
//! wants the full forward model and a `FrameTruth` to grade against; reach for this one when it
//! just needs pixels with stars at positions it chose.

use crate::internals::synthetic::patterns;
use crate::internals::synthetic::star_profiles::{StarProfile, SyntheticStar};
use common::{Buffer2, USize2};
use glam::{USizeVec2, Vec2};

/// The sky a field is drawn on.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Sky {
    /// Flat background level.
    pub(crate) level: f32,
    /// The Gaussian noise added per pixel; `None` for a noiseless sky.
    pub(crate) noise: Option<SkyNoise>,
    /// Clamp to `[0, 1]` after adding noise. Leave off when the test injects values above the
    /// ceiling afterwards — cosmic rays, for one.
    pub(crate) clamp: bool,
}

/// Gaussian noise of `sigma`, deterministic in `seed`.
#[derive(Debug, Clone, Copy)]
pub(crate) struct SkyNoise {
    pub(crate) sigma: f32,
    pub(crate) seed: u64,
}

/// Round Gaussian stars of width `sigma`, each `(centre, peak above the sky)`.
#[derive(Debug, Clone, Copy)]
pub(crate) struct GaussianStars<'a> {
    pub(crate) sigma: f32,
    pub(crate) peaks: &'a [(Vec2, f32)],
}

/// A rendered field and where its stars are.
#[derive(Debug)]
pub(crate) struct SkyField {
    pub(crate) pixels: Buffer2<f32>,
    /// Star centres rounded to the nearest pixel, for tests asserting a core survived.
    pub(crate) centers: Vec<USizeVec2>,
}

impl SkyField {
    /// Draw `stars` on `sky`.
    #[expect(
        clippy::cast_sign_loss,
        reason = "synthetic fixtures are small images with non-negative coordinates"
    )]
    pub(crate) fn render(size: USize2, sky: Sky, stars: GaussianStars<'_>) -> Self {
        let GaussianStars { sigma, peaks } = stars;
        let mut pixels = Buffer2::new_filled(size, sky.level);
        for &(center, peak) in peaks {
            SyntheticStar::new(center, peak, StarProfile::gaussian(sigma)).add_to(&mut pixels);
        }
        if let Some(SkyNoise { sigma, seed }) = sky.noise {
            patterns::add_gaussian_noise(pixels.pixels_mut(), sigma, seed);
        }
        if sky.clamp {
            for p in &mut pixels {
                *p = p.clamp(0.0, 1.0);
            }
        }
        Self {
            pixels,
            centers: peaks
                .iter()
                .map(|&(c, _)| USizeVec2::new(c.x.round() as usize, c.y.round() as usize))
                .collect(),
        }
    }
}

#[cfg(test)]
mod tests;
