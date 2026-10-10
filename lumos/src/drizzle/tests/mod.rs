mod accumulation;
mod cfa;
mod combine;
mod config;
mod geometry;
mod jacobian;
mod kernels;
mod square;
mod synthetic;

use crate::internals::prelude::*;
use crate::internals::synthetic::fixtures::star_field;

use crate::combine::config::{Combine, Normalization, StackConfig, Weighting};
use crate::drizzle::accumulator::frame_source::internals::band_scan;
use crate::drizzle::accumulator::{DrizzleAccumulator, DrizzleFrame};
use crate::drizzle::config::{DrizzleConfig, DrizzleKernel};
use crate::drizzle::deposit::Deposit;
use crate::drizzle::drizzle_result::DrizzleResult;
use crate::drizzle::error::{DrizzleConfigError, DrizzleError};
use crate::drizzle::geometry::{boxer, sgarea};
use crate::drizzle::stack::{drizzle_images, drizzle_stack};
use crate::error::FrameDimensionMismatch;
use crate::io::image::load_context::LoadContext;
use crate::io::image::pixel_flags::PixelFlags;
use crate::progress::progress_callback::ProgressCallback;
use crate::registration::transform::{Transform, WarpTransform};
use crate::stack_product::StackProduct;
use crate::stack_product::coverage::Coverage;
use crate::stack_product::quality_map::QualityMap;
use crate::stack_product::quality_planes::QualityPlanes;

/// Output pixels a drop's deposits share at most: the Lanczos-3 and Gaussian neighbourhoods are
/// 7×7.
const MAX_DEPOSITS: f32 = 49.0;

/// How far `Σ|wᵢ|` can exceed `Σwᵢ` at an interior pixel: Lanczos-3's negative lobes, about 1.3 per
/// axis. The other kernels' weights are all positive, so it is 1 for them.
const LOBE_EXCESS: f32 = 1.7;

fn accumulator(input_dims: ImageDimensions, config: DrizzleConfig) -> DrizzleAccumulator {
    DrizzleAccumulator::new(
        input_dims,
        Deposit::Channels(input_dims.channels()),
        config,
        0,
    )
    .expect("test drizzle config must be valid")
}

/// The combine that makes the drizzled frames the single-pass drizzle: a mean of every sample,
/// equally weighted and unnormalized, with the standard quality planes.
fn plain_stack() -> StackConfig {
    StackConfig {
        combine: Combine::mean(),
        weighting: Weighting::Equal,
        normalization: Normalization::None,
        ..StackConfig::light()
    }
}

/// Drizzle `frames` and combine them under [`plain_stack`].
fn drizzle_plain(
    frames: Vec<DrizzleFrame<LinearImage>>,
    config: &DrizzleConfig,
) -> Result<DrizzleResult, DrizzleError> {
    drizzle_images(
        frames,
        config,
        &plain_stack(),
        ProgressCallback::default(),
        CancelToken::never(),
    )
}

/// A drizzle config for `kernel`. `min_weight_fraction` is 0 everywhere in these tests so nothing
/// is dropped for thin coverage, and `fill_value` is 0 unless a case overrides it by struct update.
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
    acc.add_image(image, transform, pixel_weights);
    acc.finalize()
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
