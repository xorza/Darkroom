use std::f64::consts::{FRAC_PI_4, PI};

use glam::DVec2;
use imaginarium::Buffer2;

use crate::io::image::{ImageDimensions, LinearImage};
use crate::stacking::drizzle::accumulator::test_support::{
    accumulated_flux_sum, add_image as add_test_image,
};
use crate::stacking::drizzle::accumulator::{DrizzleAccumulator, DrizzleFrame};
use crate::stacking::drizzle::config::{DrizzleConfig, DrizzleKernel};
use crate::stacking::drizzle::error::{DrizzleConfigError, DrizzleError};
use crate::stacking::drizzle::geometry::{boxer, lanczos_kernel, local_jacobian, sgarea};
use crate::stacking::drizzle::stack::{drizzle_images, drizzle_stack};
use crate::stacking::product::{QualityMap, StackProduct};
use crate::stacking::progress::ProgressCallback;
use crate::stacking::registration::transform::Transform;

trait DrizzleAccumulatorTestExt {
    fn add_image(
        &mut self,
        image: LinearImage,
        transform: &Transform,
        weight: f32,
        pixel_weights: Option<&Buffer2<f32>>,
    );
}

impl DrizzleAccumulatorTestExt for DrizzleAccumulator {
    fn add_image(
        &mut self,
        image: LinearImage,
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

fn mono_image(width: usize, height: usize, pixels: Vec<f32>) -> LinearImage {
    LinearImage::from_pixels(ImageDimensions::new((width, height), 1), pixels)
}

fn constant_mono_image(width: usize, height: usize, value: f32) -> LinearImage {
    mono_image(width, height, vec![value; width * height])
}

fn assert_product_finite(product: &StackProduct) {
    for channel in 0..product.image.dimensions.channels() {
        assert!(
            product
                .image
                .channel(channel)
                .iter()
                .all(|value| value.is_finite())
        );
    }
    assert!(
        product
            .coverage
            .pixels()
            .iter()
            .all(|value| value.is_finite())
    );
    for channel in 0..product.image.dimensions.channels() {
        assert!(
            product
                .weight
                .channel(channel)
                .pixels()
                .iter()
                .all(|value| value.is_finite())
        );
        assert!(
            product
                .linear_variance
                .as_ref()
                .unwrap()
                .channel(channel)
                .pixels()
                .iter()
                .all(|value| value.is_finite())
        );
    }
}

fn drizzle_frames(
    images: Vec<LinearImage>,
    transforms: &[Transform],
) -> Vec<DrizzleFrame<LinearImage>> {
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
