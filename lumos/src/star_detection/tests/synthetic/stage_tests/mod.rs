//! Algorithm stage tests - tests individual components of the star detection pipeline.

/// Default tile size for background estimation.
pub const TILE_SIZE: usize = 64;

mod background_tests;
mod centroid_tests;
mod convolution_tests;
mod cosmic_ray_tests;
mod deblend_tests;
mod detection_tests;
