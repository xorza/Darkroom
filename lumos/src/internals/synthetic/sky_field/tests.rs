use crate::internals::synthetic::sky_field::{GaussianStars, Sky, SkyField, SkyNoise};
use common::{Size2us, Vec2us};
use glam::Vec2;

const FLAT: Sky = Sky {
    level: 0.1,
    noise: None,
    clamp: false,
};

#[test]
fn stars_sit_on_the_sky_at_their_centres() {
    let field = SkyField::render(
        Size2us::new(32, 32),
        FLAT,
        GaussianStars {
            sigma: 1.5,
            peaks: &[(Vec2::new(8.0, 20.0), 0.5)],
        },
    );
    // The peak pixel is the sky plus the amplitude times the profile's mean over it, at σ 1.5:
    // m(0) = 3·√(π/2)·erf(1/(3√2)) = 0.981786 per axis, so 0.1 + 0.5·m(0)² = 0.581952, to the
    // f32 rounding of the sum. It sits where we put it.
    assert!((field.pixels[(8, 20)] - 0.581_952).abs() <= 1e-7);
    assert_eq!(field.centers, vec![Vec2us::new(8, 20)]);
    // Far from the star, only sky.
    assert_eq!(field.pixels[(31, 0)], 0.1);
}

#[test]
fn centres_round_to_the_nearest_pixel() {
    let field = SkyField::render(
        Size2us::new(16, 16),
        FLAT,
        GaussianStars {
            sigma: 1.0,
            peaks: &[(Vec2::new(4.4, 7.6), 0.3)],
        },
    );
    assert_eq!(field.centers, vec![Vec2us::new(4, 8)]);
}

#[test]
fn noise_is_deterministic_and_clamping_is_opt_in() {
    let noisy = Sky {
        level: 0.1,
        noise: Some(SkyNoise {
            sigma: 0.05,
            seed: 7,
        }),
        clamp: false,
    };
    let spec = |sky, peaks| {
        SkyField::render(
            Size2us::new(24, 24),
            sky,
            GaussianStars { sigma: 1.5, peaks },
        )
    };

    // Same seed, same field.
    assert_eq!(
        spec(noisy, &[]).pixels.pixels(),
        spec(noisy, &[]).pixels.pixels()
    );
    // Noise moves it off the flat sky.
    assert!(spec(noisy, &[]).pixels.pixels().iter().any(|&v| v != 0.1));

    // A star brighter than the ceiling survives unclamped and is cut when clamping is on.
    let bright = [(Vec2::splat(12.0), 2.0)];
    let unclamped = spec(FLAT, &bright);
    let clamped = spec(
        Sky {
            clamp: true,
            ..FLAT
        },
        &bright,
    );
    assert!(unclamped.pixels[(12, 12)] > 1.0);
    assert_eq!(clamped.pixels[(12, 12)], 1.0);
}
