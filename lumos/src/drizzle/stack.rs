//! Drizzle entry points: from paths, or from frames already in memory.
//!
//! Each frame is drizzled onto the output grid on its own, as Siril does: its drops' weighted mean,
//! their summed weight and their Kish size, the noise factor of the mean. The drizzled frames then
//! go through the combine every stack takes — normalization, weighting, rejection and the quality
//! planes — with each frame's drop weight multiplying its frame weight in the mean, as Siril's
//! (`median_and_mean.c`, `n *= dstack[frame]`) and DrizzlePac's weights do. Without rejection, and
//! with equal frame weights, that is the single-pass drizzle.
//!
//! The path form keeps one decoded input resident: a frame is loaded, drizzled, stored, and
//! dropped before the next is read.

use std::iter;
use std::path::Path;

use common::CancelToken;
use imaginarium::Buffer2;

use crate::combine::config::StackConfig;
use crate::combine::error::StackError;
use crate::combine::stack::stack_stored_frames;
use crate::drizzle::accumulator::{DrizzleAccumulator, DrizzleFrame, DrizzledPlanes};
use crate::drizzle::config::DrizzleConfig;
use crate::drizzle::drizzle_result::DrizzleResult;
use crate::drizzle::drizzle_tier::{DrizzleTier, FrameOrigin};
use crate::drizzle::error::DrizzleError;
use crate::frame_store::frame_quality::FrameQuality;
use crate::frame_store::frame_stats::FrameStats;
use crate::ingest::ingest_run::IngestRun;
use crate::io::image::error::ImageError;
use crate::io::image::image_dimensions::ImageDimensions;
use crate::io::image::linear::LinearImage;
use crate::io::image::load_context::LoadContext;
use crate::progress::progress_callback::ProgressCallback;
use crate::progress::stacking_progress::StackingStage;

fn load_drizzle_frame<P: AsRef<Path>>(
    frame: DrizzleFrame<P>,
    context: &LoadContext,
) -> Result<DrizzleFrame<LinearImage>, DrizzleError> {
    let DrizzleFrame {
        source,
        warp,
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
        warp,
        pixel_weight_map,
    })
}

/// Drizzle images from disk with per-frame warps, and combine them under `stack`.
///
/// Streams frames one at a time (only one input image is resident at a time). To drizzle frames
/// already held in memory, use [`drizzle_images`].
///
/// # Returns
///
/// The drizzled product — its image in the input's surface-brightness units, see
/// [`DrizzleConfig::scale`] — with the quality planes `stack` asks for, and the count of input
/// pixels a SIP warp could not place.
///
/// # Errors
///
/// Returns an error for an invalid configuration, missing frames, image loading failures,
/// inconsistent image dimensions, a non-finite sample, invalid pixel weights, a failed combine, or
/// cancellation.
#[expect(
    clippy::needless_pass_by_value,
    reason = "every stacking entry takes its progress and cancel token the same way, by value"
)]
pub fn drizzle_stack<P: AsRef<Path>>(
    frames: Vec<DrizzleFrame<P>>,
    drizzle: &DrizzleConfig,
    stack: &StackConfig,
    progress: ProgressCallback,
    cancel: CancelToken,
) -> Result<DrizzleResult, DrizzleError> {
    let frame_count = frames.len();
    let run = IngestRun::new(&drizzle.ingest, cancel.clone());
    let context = run.context.clone();
    // Lazy, so only the frame currently being drizzled is resident.
    let loaded = frames
        .into_iter()
        .map(move |frame| load_drizzle_frame(frame, &context));
    accumulate(
        loaded,
        frame_count,
        FrameOrigin::Files,
        drizzle,
        stack,
        &run,
        &progress,
    )
}

/// Drizzle frames already held in memory, and combine them under `stack`.
///
/// In-memory counterpart to [`drizzle_stack`]: skips the per-frame disk load when the caller
/// already owns the decoded frames. The frames are consumed.
///
/// # Errors
///
/// As [`drizzle_stack`], but for the loads.
#[expect(
    clippy::needless_pass_by_value,
    reason = "every stacking entry takes its progress and cancel token the same way, by value"
)]
pub fn drizzle_images(
    frames: Vec<DrizzleFrame<LinearImage>>,
    drizzle: &DrizzleConfig,
    stack: &StackConfig,
    progress: ProgressCallback,
    cancel: CancelToken,
) -> Result<DrizzleResult, DrizzleError> {
    let frame_count = frames.len();
    let run = IngestRun::new(&drizzle.ingest, cancel);
    accumulate(
        frames.into_iter().map(Ok),
        frame_count,
        FrameOrigin::Memory,
        drizzle,
        stack,
        &run,
        &progress,
    )
}

/// Drizzle every frame on its own, store it, combine the stored frames, and gate the product —
/// the body both entry points share.
///
/// Frames arrive as a lazy fallible iterator so the path entry keeps one input image resident: an
/// item is produced, drizzled, stored, and dropped before the next is loaded.
fn accumulate(
    mut frames: impl Iterator<Item = Result<DrizzleFrame<LinearImage>, DrizzleError>>,
    frame_count: usize,
    origin: FrameOrigin,
    drizzle: &DrizzleConfig,
    stack: &StackConfig,
    run: &IngestRun,
    progress: &ProgressCallback,
) -> Result<DrizzleResult, DrizzleError> {
    if frame_count == 0 {
        return Err(DrizzleError::NoFrames);
    }
    let cancel = &run.context.cancel;
    // Before the first frame is pulled: for the path entry that is a full decode, spent on a run
    // the configuration already dooms.
    drizzle.validate()?;
    stack.validate().map_err(StackError::from)?;

    let first = frames.next().expect("frame_count is non-zero")?;
    let input_dims = first.source.dimensions();
    tracing::info!(
        ?origin,
        frame_count,
        input_width = input_dims.width(),
        input_height = input_dims.height(),
        channels = input_dims.channels(),
        output_scale = drizzle.scale,
        pixfrac = drizzle.pixfrac,
        kernel = ?drizzle.kernel,
        "Starting drizzle stacking"
    );
    let output = ImageDimensions::new(
        drizzle.output_size(input_dims.size()),
        input_dims.channels(),
    );
    let tier = DrizzleTier::for_run(input_dims, output, frame_count, origin, stack, run)?;

    let mut stored = Vec::with_capacity(frame_count);
    // Every frame's drops' weight at each output pixel, before any frame weight: how much signal
    // landed there, which the fill gate reads.
    let mut depth = Buffer2::new_default(output.width(), output.height());
    let mut unconverged_points = 0;
    let mut metadata = None;
    for (index, frame) in iter::once(Ok(first)).chain(frames).enumerate() {
        // Between frames, so a cancelled run stops before loading and drizzling the next one.
        if cancel.is_cancelled() {
            return Err(DrizzleError::Cancelled);
        }
        let frame = frame?;
        // Measured on the frame as it arrives, before its drops are spread: the drizzled pixels
        // average several, and would understate it.
        let source_stats = FrameStats::measure(&frame.source);
        let mut accumulator = DrizzleAccumulator::new(input_dims, drizzle.clone(), index)?;
        accumulator.add_frame(&frame)?;
        unconverged_points += accumulator.unconverged_points();
        let frame_metadata = frame.source.metadata.clone().warped(
            |position| frame.warp.apply(accumulator.reference_point(position)),
            output.size(),
        );
        drop(frame);
        let DrizzledPlanes {
            pixels,
            weight,
            confidence,
            flags,
        } = accumulator.into_planes();
        for (depth, &weight) in depth.pixels_mut().iter_mut().zip(weight.pixels()) {
            *depth += weight;
        }
        metadata.get_or_insert_with(|| frame_metadata.clone());
        let mut image =
            LinearImage::from_planar_channels(output, pixels.into_iter().map(Buffer2::into_vec));
        image.metadata = frame_metadata;
        image.flags = flags;
        let quality = FrameQuality::Drizzled { weight, confidence };
        stored.push(tier.store(image, quality, source_stats)?);
        progress.report(index + 1, frame_count, StackingStage::Drizzling);
    }

    let mut product = stack_stored_frames(
        stored,
        tier.cache_tier(),
        output,
        metadata.expect("a frame was drizzled"),
        stack,
        progress.clone(),
        cancel.clone(),
    )?;
    // A pixel below the gate is held at the fill value: what the frames left there is too thin a
    // share of the deepest pixel's to stand. Floored at the smallest positive float, so "reached at
    // all" and "reached enough" are one comparison.
    let deepest = depth.pixels().iter().copied().fold(0.0f32, f32::max);
    let threshold = (drizzle.min_weight_fraction * deepest).max(f32::MIN_POSITIVE);
    let depth = depth.pixels();
    product.fill_where(|index| depth[index] < threshold, drizzle.fill_value);
    Ok(DrizzleResult {
        product,
        unconverged_points,
    })
}
