//! BackgroundEstimate generation for testing.
//!
//! Provides utilities to create BackgroundEstimate instances for benchmarks and tests.

use crate::stacking::star_detection::background::estimate::BackgroundEstimate;
use imaginarium::Buffer2;

/// Create a uniform BackgroundEstimate with constant background and noise values.
pub(crate) fn uniform(
    width: usize,
    height: usize,
    background: f32,
    noise: f32,
) -> BackgroundEstimate {
    let mut bg_buf = Buffer2::new_default(width, height);
    let mut noise_buf = Buffer2::new_default(width, height);
    bg_buf.fill(background);
    noise_buf.fill(noise);
    BackgroundEstimate {
        background: bg_buf,
        noise: noise_buf,
    }
}

#[cfg(test)]
mod tests {
    use crate::testing::synthetic::background_map::*;

    #[test]
    fn test_uniform() {
        let bg = uniform(100, 100, 0.1, 0.01);
        assert_eq!(bg.background.width(), 100);
        assert_eq!(bg.background.height(), 100);
        assert!((bg.background[(50, 50)] - 0.1).abs() < 1e-6);
        assert!((bg.noise[(50, 50)] - 0.01).abs() < 1e-6);
    }
}
