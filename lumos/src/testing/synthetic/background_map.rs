//! BackgroundEstimate generation for testing.
//!
//! Provides utilities to create BackgroundEstimate instances for benchmarks and tests.

use crate::math::size2us::Size2us;
use crate::stacking::star_detection::background::background_estimate::BackgroundEstimate;
use crate::stacking::star_detection::config::background_config::BackgroundConfig;
use crate::stacking::star_detection::resources::DetectionResources;
use imaginarium::Buffer2;

/// Create a uniform BackgroundEstimate with constant background and noise values.
pub(crate) fn uniform(size: Size2us, background: f32, noise: f32) -> BackgroundEstimate {
    let mut bg_buf = Buffer2::new_default(size.width, size.height);
    let mut noise_buf = Buffer2::new_default(size.width, size.height);
    bg_buf.fill(background);
    noise_buf.fill(noise);
    BackgroundEstimate {
        background: bg_buf,
        noise: noise_buf,
    }
}

/// Run the real background estimator over `pixels`, managing the buffer pool for the caller.
///
/// The counterpart to [`uniform`]: that one hands back a flat map, this one measures the image.
pub(crate) fn estimate(pixels: &Buffer2<f32>, config: &BackgroundConfig) -> BackgroundEstimate {
    let mut pool = DetectionResources::new(Size2us::new(pixels.width(), pixels.height()));
    BackgroundEstimate::estimate(pixels, config, &mut pool)
}

#[cfg(test)]
mod tests {
    use crate::testing::synthetic::background_map::*;

    #[test]
    fn uniform_fills_both_the_background_and_noise_planes() {
        let bg = uniform(Size2us::new(100, 100), 0.1, 0.01);
        assert_eq!(bg.background.width(), 100);
        assert_eq!(bg.background.height(), 100);
        assert!((bg.background[(50, 50)] - 0.1).abs() < 1e-6);
        assert!((bg.noise[(50, 50)] - 0.01).abs() < 1e-6);
    }
}
