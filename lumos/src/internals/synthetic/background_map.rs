//! `BackgroundEstimate` generation for testing.
//!
//! Provides utilities to create `BackgroundEstimate` instances for benchmarks and tests.

use crate::math::size2us::Size2us;
use crate::scratch_pool::Pooled;
use crate::star_detection::background::background_estimate::BackgroundEstimate;
use crate::star_detection::background::sky_noise::{SkyNoise, noise_floor_for};
use crate::star_detection::detection_plane::{PlaneSpec, Prefilter};
use crate::star_detection::refined_background::RefinedBackground;
use crate::star_detection::resources::DetectionResources;
use crate::star_detection::star_detection_config::background_config::BackgroundConfig;
use imaginarium::Buffer2;

/// Create a uniform `BackgroundEstimate` with constant background and noise values.
pub(crate) fn uniform(size: Size2us, background: f32, noise: f32) -> BackgroundEstimate {
    BackgroundEstimate {
        background: Pooled::loose(
            Buffer2::new_filled(size.width, size.height, background),
            size,
        ),
        sky: SkyNoise {
            noise: Pooled::loose(Buffer2::new_filled(size.width, size.height, noise), size),
            // The map is uniform, so the frame's typical σ is `noise` itself and the estimator's
            // median-tile-σ derivation reduces to exactly this.
            floor: noise_floor_for(noise),
        },
    }
}

/// Run the real background estimator over `pixels`, managing the buffer pool for the caller,
/// with `config`'s refinement when it asks for one, on the unfiltered residual.
///
/// The counterpart to [`uniform`]: that one hands back a flat map, this one measures the image.
pub(crate) fn estimate(pixels: &Buffer2<f32>, config: &BackgroundConfig) -> BackgroundEstimate {
    let mut pool = DetectionResources::new(Size2us::from(pixels));
    estimate_in(pixels, config, &mut pool)
}

/// [`estimate`] with the caller's pool, so a bench or a reuse test can recycle its planes.
pub(crate) fn estimate_in(
    pixels: &Buffer2<f32>,
    config: &BackgroundConfig,
    pool: &mut DetectionResources,
) -> BackgroundEstimate {
    RefinedBackground::measure(
        pixels,
        PlaneSpec {
            prefilter: Prefilter::None,
            matched: None,
            mask: None,
            background: config,
        },
        pool,
    )
    .estimate
}

#[cfg(test)]
mod tests {
    use crate::internals::synthetic::background_map::*;

    #[test]
    fn uniform_fills_both_the_background_and_noise_planes() {
        let bg = uniform(Size2us::new(100, 100), 0.1, 0.01);
        assert_eq!(bg.background.width(), 100);
        assert_eq!(bg.background.height(), 100);
        assert!((bg.background[(50, 50)] - 0.1).abs() < 1e-6);
        assert!((bg.sky.noise[(50, 50)] - 0.01).abs() < 1e-6);
    }
}
