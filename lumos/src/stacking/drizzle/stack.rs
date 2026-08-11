//! Drizzle entry points: from paths, or from frames already in memory.
//!
//! Both feed one accumulation body. The path form keeps exactly one decoded frame resident by
//! handing that body a lazy fallible iterator, so a frame is loaded, distributed into the grid,
//! and dropped before the next one is read.

use std::path::Path;

use common::CancelToken;

use crate::io::image::error::ImageError;
use crate::io::image::linear::LinearImage;
use crate::io::image::load_context::LoadContext;
use crate::stacking::drizzle::accumulator::{DrizzleAccumulator, DrizzleFrame};
use crate::stacking::drizzle::config::DrizzleConfig;
use crate::stacking::drizzle::error::DrizzleError;
use crate::stacking::progress::{ProgressCallback, StackingStage};
use crate::stacking::stack_product::StackProduct;

fn load_drizzle_frame<P: AsRef<Path>>(
    frame: DrizzleFrame<P>,
    context: &LoadContext,
) -> Result<DrizzleFrame<LinearImage>, DrizzleError> {
    let DrizzleFrame {
        source,
        transform,
        weight,
        pixel_weight_map,
    } = frame;
    let image = match LinearImage::from_file(source.as_ref(), context) {
        Ok(image) => image,
        // Cancellation is the run stopping, not this file failing, so it leaves the load error
        // behind and reports as the drizzle's own.
        Err(ImageError::Cancelled { .. }) => return Err(DrizzleError::Cancelled),
        Err(error) => return Err(DrizzleError::ImageLoad(error)),
    };
    Ok(DrizzleFrame {
        source: image,
        transform,
        weight,
        pixel_weight_map,
    })
}

/// Drizzle stack images from disk with per-frame transforms.
///
/// Streams frames one at a time (only one input image is resident at a time). To drizzle frames
/// already held in memory, use [`drizzle_images`].
///
/// # Arguments
///
/// * `frames` - Paths bundled with their transform and optional quality weights
/// * `config` - Drizzle configuration
/// * `context` - Resource limits and image-load policy. Its `cancel` field is **ignored** — the
///   `cancel` argument governs the whole run, decode included, so cancellation reads the same
///   here as on every other stacking entry point
/// * `progress` - Progress callback
/// * `cancel` - Cooperative cancellation, polled between frames and inside each decode
///
/// # Returns
///
/// The drizzled result: image plus coverage, weight (`Σwᵢ`), and linear-variance
/// (`Σwᵢ²/(Σwᵢ)²`) maps.
///
/// # Errors
///
/// Returns an error for invalid configuration, missing frames, image loading failures,
/// inconsistent image dimensions, invalid frame weights, or cancellation.
pub fn drizzle_stack<P: AsRef<Path>>(
    frames: Vec<DrizzleFrame<P>>,
    config: &DrizzleConfig,
    context: &LoadContext,
    progress: ProgressCallback,
    cancel: CancelToken,
) -> Result<StackProduct, DrizzleError> {
    let frame_count = frames.len();
    // The decoders poll cancellation off the context, so the run's token has to reach them —
    // the caller supplies decode policy, this supplies the token.
    let context = LoadContext {
        cancel: cancel.clone(),
        ..context.clone()
    };
    // Lazy, so only the frame currently being accumulated is resident.
    let loaded = frames
        .into_iter()
        .map(|frame| load_drizzle_frame(frame, &context));
    accumulate(loaded, frame_count, config, progress, &cancel, "paths")
}

/// Drizzle stack frames already held in memory.
///
/// In-memory counterpart to [`drizzle_stack`]: skips the per-frame disk load when the caller
/// already owns the decoded frames. The frames are consumed.
///
/// # Errors
///
/// Returns an error for invalid configuration, missing frames, inconsistent image dimensions,
/// invalid frame weights, or cancellation.
pub fn drizzle_images(
    frames: Vec<DrizzleFrame<LinearImage>>,
    config: &DrizzleConfig,
    progress: ProgressCallback,
    cancel: CancelToken,
) -> Result<StackProduct, DrizzleError> {
    let frame_count = frames.len();
    accumulate(
        frames.into_iter().map(Ok),
        frame_count,
        config,
        progress,
        &cancel,
        "memory",
    )
}

/// Accumulate every frame into one drizzled product — the body both entry points share.
///
/// Frames arrive as a lazy fallible iterator so the path-based entry keeps exactly one input
/// image resident: an item is produced, distributed into the accumulator, and dropped before the
/// next is loaded.
fn accumulate(
    mut frames: impl Iterator<Item = Result<DrizzleFrame<LinearImage>, DrizzleError>>,
    frame_count: usize,
    config: &DrizzleConfig,
    progress: ProgressCallback,
    cancel: &CancelToken,
    source: &'static str,
) -> Result<StackProduct, DrizzleError> {
    if frame_count == 0 {
        return Err(DrizzleError::NoFrames);
    }

    // The accumulator is sized from the first frame, so it has to be in hand before the loop. It
    // validates the config as it is built, which is the only place that check belongs.
    let first = frames.next().expect("frame_count is non-zero")?;
    let input_dims = first.source.dimensions();
    tracing::info!(
        source,
        frame_count,
        input_width = input_dims.width(),
        input_height = input_dims.height(),
        channels = input_dims.channels(),
        output_scale = config.scale,
        pixfrac = config.pixfrac,
        kernel = ?config.kernel,
        "Starting drizzle stacking"
    );

    let mut accumulator = DrizzleAccumulator::new(input_dims, config.clone())?;
    accumulator.add_frame(first)?;
    progress.report(1, frame_count, StackingStage::Drizzling);

    for (index, frame) in frames.enumerate() {
        // Between frames, so a cancelled run stops before loading and distributing the next one.
        if cancel.is_cancelled() {
            return Err(DrizzleError::Cancelled);
        }
        accumulator.add_frame(frame?)?;
        progress.report(index + 2, frame_count, StackingStage::Drizzling);
    }

    Ok(accumulator.finalize())
}
