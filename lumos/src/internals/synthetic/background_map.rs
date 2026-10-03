//! `BackgroundEstimate` generation for testing.
//!
//! Provides utilities to create `BackgroundEstimate` instances for benchmarks and tests.

use crate::math::size2us::Size2us;
use crate::star_detection::background::background_estimate::{
    BackgroundEstimate, Refinement, noise_floor_for,
};
use crate::star_detection::config::background_config::{BackgroundConfig, BackgroundRefinement};
use crate::star_detection::detection_plane::PlaneFilters;
use crate::star_detection::resources::DetectionResources;
use imaginarium::Buffer2;

/// Create a uniform `BackgroundEstimate` with constant background and noise values.
pub(crate) fn uniform(size: Size2us, background: f32, noise: f32) -> BackgroundEstimate {
    let mut bg_buf = Buffer2::new_default(size.width, size.height);
    let mut noise_buf = Buffer2::new_default(size.width, size.height);
    bg_buf.fill(background);
    noise_buf.fill(noise);
    BackgroundEstimate {
        background: bg_buf,
        noise: noise_buf,
        // The map is uniform, so the frame's typical σ is `noise` itself and the estimator's
        // median-tile-σ derivation reduces to exactly this.
        noise_floor: noise_floor_for(noise),
    }
}

/// Run the real background estimator over `pixels`, managing the buffer pool for the caller,
/// with `config`'s refinement when it asks for one, on the unfiltered residual.
///
/// The counterpart to [`uniform`]: that one hands back a flat map, this one measures the image.
pub(crate) fn estimate(pixels: &Buffer2<f32>, config: &BackgroundConfig) -> BackgroundEstimate {
    let mut pool = DetectionResources::new(Size2us::new(pixels.width(), pixels.height()));
    estimate_in(pixels, config, &mut pool)
}

/// [`estimate`] with the caller's pool, so a bench or a reuse test can recycle its planes.
pub(crate) fn estimate_in(
    pixels: &Buffer2<f32>,
    config: &BackgroundConfig,
    pool: &mut DetectionResources,
) -> BackgroundEstimate {
    let estimate = BackgroundEstimate::estimate(pixels, None, config, pool);
    let BackgroundRefinement::Iterative {
        iterations,
        mask_dilation,
        mask_sigma,
    } = config.refinement
    else {
        return estimate;
    };
    let refined = estimate.refine(
        pixels,
        Refinement {
            iterations,
            mask_dilation,
            mask_sigma,
        },
        PlaneFilters {
            median: false,
            matched: None,
            mask: None,
        },
        config,
        pool,
    );
    pool.release_bit(refined.sources);
    refined.estimate
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
        assert!((bg.noise[(50, 50)] - 0.01).abs() < 1e-6);
    }
}
