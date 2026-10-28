mod accumulation;
mod cfa;
mod combine;
mod config;
mod jacobian;
mod kernels;
mod square;
mod synthetic;

use common::Buffer2;
use imaginarium::PlanarDesc;

use crate::combine::normalization::Normalization;
use crate::combine::stack_config::StackConfig;
use crate::combine::stack_config::combine::Combine;
use crate::combine::stack_config::weighting::Weighting;
use crate::drizzle::accumulator::frame_source::internals::band_scan;
use crate::drizzle::accumulator::internals::DrizzleAccumulator;
use crate::drizzle::accumulator::kernel_plan::DropReach;
use crate::drizzle::drizzle_config::DrizzleConfig;
use crate::drizzle::drizzle_frame::DrizzleFrame;
use crate::drizzle::drizzle_kernel::DrizzleKernel;
use crate::drizzle::error::{DrizzleConfigError, DrizzleError};
use crate::image::error::FrameDimensionMismatch;
use crate::image::flags::pixel_flags::PixelFlags;
use crate::image::flags::quality_map::QualityMap;
use crate::image::linear_image::LinearImage;
use crate::ingest::ingest_config::IngestConfig;
use crate::internals::images::mono_image;
use crate::internals::synthetic::fixtures::star_field;
use crate::io::load_ctx::LoadCtx;
use crate::progress::progress_monitor::ProgressMonitor;
use crate::registration::transform::Transform;
use crate::registration::warp_transform::WarpTransform;
use crate::stack_product::StackProduct;
use crate::stack_product::coverage::Coverage;
use crate::stack_product::quality_planes::QualityPlanes;
use common::USize2;

/// Output pixels a drop's deposits share at most: the Lanczos-3 and Gaussian neighbourhoods are
/// 7×7.
const MAX_DEPOSITS: f32 = 49.0;

/// How far `Σ|wᵢ|` can exceed `Σwᵢ` at an interior pixel: Lanczos-3's negative lobes, about 1.3 per
/// axis. The other kernels' weights are all positive, so it is 1 for them.
const LOBE_EXCESS: f32 = 1.7;

/// An accumulator for frames of `input_desc`, each channel reaching its own.
fn accumulator(input_desc: PlanarDesc, config: DrizzleConfig) -> DrizzleAccumulator {
    let blank = LinearImage::from_planar_channels(
        input_desc,
        (0..input_desc.channel_count()).map(|_| vec![0.0; input_desc.pixel_count()]),
    );
    DrizzleAccumulator::new(&blank, config, 0).expect("test drizzle config must be valid")
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
) -> Result<StackProduct, DrizzleError> {
    DrizzleConfig {
        stack: plain_stack(),
        ..config.clone()
    }
    .drizzle_images(
        frames,
        &IngestConfig::default(),
        ProgressMonitor::unattended(),
    )
}

/// A drizzle config for `kernel`. `min_weight_fraction` is 0 everywhere in these tests so nothing
/// is dropped for thin coverage, `fill_value` is 0 unless a case overrides it by struct update, and
/// the frames combine under [`plain_stack`].
fn kernel_config(kernel: DrizzleKernel, scale: f32, pixfrac: f32) -> DrizzleConfig {
    DrizzleConfig {
        scale,
        pixfrac,
        kernel,
        fill_value: 0.0,
        min_weight_fraction: 0.0,
        stack: plain_stack(),
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

/// Drizzle one frame and finalize.
fn drizzle_one(config: DrizzleConfig, frame: &DrizzleFrame<LinearImage>) -> StackProduct {
    let mut acc = DrizzleAccumulator::new(&frame.source, config, 0)
        .expect("test drizzle config must be valid");
    acc.add_frame(frame)
        .expect("test frame must be coherent with the accumulator");
    acc.finalize()
}

fn constant_image(size: USize2, value: f32) -> LinearImage {
    mono_image(size, vec![value; size.pixel_count()])
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
