mod accumulation;
mod config;
mod geometry;
mod jacobian;
mod kernels;
mod square;
mod synthetic;

use crate::testing::prelude::*;
use crate::testing::synthetic::fixtures::star_field;
use std::f64::consts::{FRAC_PI_4, PI};

use crate::error::FrameDimensionMismatch;
use crate::io::image::load_context::LoadContext;
use crate::stacking::drizzle::accumulator::frame_source::internals::input_rows;
use crate::stacking::drizzle::accumulator::{DrizzleAccumulator, DrizzleFrame};
use crate::stacking::drizzle::config::{DrizzleConfig, DrizzleKernel};
use crate::stacking::drizzle::error::{DrizzleConfigError, DrizzleError};
use crate::stacking::drizzle::geometry::{boxer, sgarea};
use crate::stacking::drizzle::stack::{drizzle_images, drizzle_stack};
use crate::stacking::progress::ProgressCallback;
use crate::stacking::registration::transform::{Transform, WarpTransform};
use crate::stacking::stack_product::StackProduct;
use crate::stacking::stack_product::coverage::Coverage;
use crate::stacking::stack_product::quality_map::QualityMap;
use crate::stacking::stack_product::quality_planes::QualityPlanes;

fn accumulator(input_dims: ImageDimensions, config: DrizzleConfig) -> DrizzleAccumulator {
    DrizzleAccumulator::new(input_dims, config).expect("test drizzle config must be valid")
}

/// A drizzle config for `kernel`. `min_weight_fraction` is 0 everywhere in these tests so nothing is
/// dropped for thin coverage, and `fill_value` is 0 unless a case overrides it by struct update.
fn kernel_config(kernel: DrizzleKernel, scale: f32, pixfrac: f32) -> DrizzleConfig {
    DrizzleConfig {
        scale,
        pixfrac,
        kernel,
        fill_value: 0.0,
        min_weight_fraction: 0.0,
        ..Default::default()
    }
}

/// Drizzle one mono frame of `side`×`side` and finalize.
fn drizzle_one(
    side: usize,
    config: DrizzleConfig,
    image: LinearImage,
    transform: &Transform,
    pixel_weights: Option<&Buffer2<f32>>,
) -> StackProduct {
    let mut acc = accumulator(ImageDimensions::new((side, side), 1), config);
    acc.add_image(image, transform, 1.0, pixel_weights);
    acc.finalize().product
}

fn mono_image(size: Size2us, pixels: Vec<f32>) -> LinearImage {
    LinearImage::from_pixels(ImageDimensions::new(size, 1), pixels)
}

fn constant_mono_image(size: Size2us, value: f32) -> LinearImage {
    mono_image(size, vec![value; size.pixel_count()])
}

fn assert_product_finite(product: &StackProduct) {
    for channel in 0..product.image.channels() {
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
            .as_ref()
            .unwrap()
            .to_plane()
            .pixels()
            .iter()
            .all(|value| value.is_finite())
    );
    for channel in 0..product.image.channels() {
        assert!(
            product
                .weight
                .as_ref()
                .unwrap()
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
        .map(|(source, transform)| DrizzleFrame::new(source, warp_of(transform)))
        .collect()
}

/// The registration warp a fixture's input-to-reference `transform` stands for: its inverse, the
/// direction registration reports.
fn warp_of(transform: Transform) -> WarpTransform {
    WarpTransform::new(transform.inverse())
}
