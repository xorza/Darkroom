//! `BgEstimate` generation for testing.
//!
//! Provides utilities to create `BgEstimate` instances for benchmarks and tests.

use imaginarium::Buffer2;

use crate::math::size2us::Size2us;
use crate::scratch_pool::Pooled;
use crate::star_detection::bg::bg_estimate::BgEstimate;
use crate::star_detection::bg::sky_noise::{SkyNoise, noise_floor_for};
use crate::star_detection::detection_plane::{PlaneSpec, Prefilter};
use crate::star_detection::refined_bg::RefinedBg;
use crate::star_detection::resources::DetectionResources;
use crate::star_detection::star_detection_config::bg_config::BgConfig;

/// Create a uniform `BgEstimate` with constant background and noise values.
pub(crate) fn uniform(size: Size2us, bg: f32, noise: f32) -> BgEstimate {
    BgEstimate {
        bg: Pooled::loose(Buffer2::new_filled(size.width, size.height, bg), size),
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
pub(crate) fn estimate(pixels: &Buffer2<f32>, config: &BgConfig) -> BgEstimate {
    let mut pool = DetectionResources::new(Size2us::from(pixels));
    estimate_in(pixels, config, &mut pool)
}

/// [`estimate`] with the caller's pool, so a bench or a reuse test can recycle its planes.
pub(crate) fn estimate_in(
    pixels: &Buffer2<f32>,
    config: &BgConfig,
    pool: &mut DetectionResources,
) -> BgEstimate {
    RefinedBg::measure(
        pixels,
        PlaneSpec {
            prefilter: Prefilter::None,
            matched: None,
            mask: None,
            bg: config,
        },
        pool,
    )
    .estimate
}

#[cfg(test)]
mod tests {
    use crate::internals::synthetic::bg_map::*;

    #[test]
    fn uniform_fills_both_the_background_and_noise_planes() {
        let bg = uniform(Size2us::new(100, 100), 0.1, 0.01);
        assert_eq!(bg.bg.width(), 100);
        assert_eq!(bg.bg.height(), 100);
        assert!((bg.bg[(50, 50)] - 0.1).abs() < 1e-6);
        assert!((bg.sky.noise[(50, 50)] - 0.01).abs() < 1e-6);
    }
}
