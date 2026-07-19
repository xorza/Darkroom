use std::f64::consts::{FRAC_PI_4, PI};

use glam::DVec2;
use imaginarium::Buffer2;

use crate::io::astro_image::{AstroImage, ImageDimensions};
use crate::stacking::drizzle::accumulator::test_support::{
    accumulated_flux_sum, add_image as add_test_image,
};
use crate::stacking::drizzle::accumulator::{DrizzleAccumulator, DrizzleFrame};
use crate::stacking::drizzle::config::{DrizzleConfig, DrizzleKernel};
use crate::stacking::drizzle::error::{DrizzleConfigError, DrizzleError};
use crate::stacking::drizzle::geometry::{boxer, lanczos_kernel, local_jacobian, sgarea};
use crate::stacking::drizzle::stack::{drizzle_images, drizzle_stack};
use crate::stacking::progress::ProgressCallback;
use crate::stacking::registration::transform::Transform;

trait DrizzleAccumulatorTestExt {
    fn add_image(
        &mut self,
        image: AstroImage,
        transform: &Transform,
        weight: f32,
        pixel_weights: Option<&Buffer2<f32>>,
    );
}

impl DrizzleAccumulatorTestExt for DrizzleAccumulator {
    fn add_image(
        &mut self,
        image: AstroImage,
        transform: &Transform,
        weight: f32,
        pixel_weights: Option<&Buffer2<f32>>,
    ) {
        add_test_image(self, image, transform, weight, pixel_weights);
    }
}

fn accumulator(input_dims: ImageDimensions, config: DrizzleConfig) -> DrizzleAccumulator {
    DrizzleAccumulator::new(input_dims, config).expect("test drizzle config must be valid")
}

fn drizzle_frames(
    images: Vec<AstroImage>,
    transforms: &[Transform],
) -> Vec<DrizzleFrame<AstroImage>> {
    assert_eq!(images.len(), transforms.len());
    images
        .into_iter()
        .zip(transforms.iter().copied())
        .map(|(source, transform)| DrizzleFrame::new(source, transform))
        .collect()
}

mod accumulation;
mod config;
mod geometry;
mod jacobian;
mod kernels;
mod square;
