mod accumulation;
mod config;
mod geometry;
mod jacobian;
mod kernels;
mod square;
mod synthetic;

use crate::testing::prelude::*;
use crate::testing::synthetic::fixtures::star_field;

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

/// [`kernel_config`] at the sampling `kernel` is usually run at: scale 2 and pixfrac 0.8, except
/// Lanczos, which is defined only at scale 1 and pixfrac 1.
fn usual_config(kernel: DrizzleKernel) -> DrizzleConfig {
    match kernel {
        DrizzleKernel::Lanczos => kernel_config(kernel, 1.0, 1.0),
        _ => kernel_config(kernel, 2.0, 0.8),
    }
}

/// Drizzle one mono frame of `size` and finalize.
fn drizzle_one(
    size: Size2us,
    config: DrizzleConfig,
    image: LinearImage,
    transform: &Transform,
    pixel_weights: Option<&Buffer2<f32>>,
) -> StackProduct {
    let mut acc = accumulator(ImageDimensions::new(size, 1), config);
    acc.add_image(image, transform, 1.0, pixel_weights);
    acc.finalize().product
}

fn constant_image(size: Size2us, value: f32) -> LinearImage {
    gray_image(size, vec![value; size.pixel_count()])
}

/// The one weight plane drizzle shares between channels.
fn weight_plane(product: &StackProduct) -> &Buffer2<f32> {
    product
        .weight
        .as_ref()
        .expect("weight was requested")
        .channel(0)
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
