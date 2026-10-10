//! Drizzle entry points: from paths, or from frames already in memory, of demosaiced or mono
//! frames or of calibrated mosaics.
//!
//! Each frame is drizzled onto the output grid on its own, as Siril does: its drops' weighted mean,
//! their summed weight and their Kish size, the noise factor of the mean. The drizzled frames then
//! go through the combine every stack takes — normalization, weighting, rejection and the quality
//! planes — with each frame's drop weight multiplying its frame weight in the mean, as Siril's
//! (`median_and_mean.c`, `n *= dstack[frame]`) and DrizzlePac's weights do. Without rejection, and
//! with equal frame weights, that is the single-pass drizzle.
//!
//! A mosaic drizzles each photosite into the channel of its colour alone, so its colour planes come
//! out with no colour interpolated from another: a demosaic, and the second interpolation a warp
//! would add, are both skipped. Each channel then has its own weights, and the combine gathers it
//! by them.
//!
//! The path form keeps one decoded input resident: a frame is loaded, drizzled, stored, and
//! dropped before the next is read.

use std::iter;
use std::path::Path;

use arrayvec::ArrayVec;
use common::CancelToken;
use imaginarium::Buffer2;

use crate::combine::config::StackConfig;
use crate::combine::error::StackError;
use crate::combine::stack::stack_stored_frames;
use crate::drizzle::accumulator::{DrizzleAccumulator, DrizzleFrame, DrizzledPlanes};
use crate::drizzle::config::DrizzleConfig;
use crate::drizzle::deposit::Deposit;
use crate::drizzle::drizzle_result::DrizzleResult;
use crate::drizzle::drizzle_tier::{DrizzleTier, FrameOrigin};
use crate::drizzle::error::DrizzleError;
use crate::frame_store::frame_quality::FrameQuality;
use crate::frame_store::frame_stats::FrameStats;
use crate::frame_store::stackable_image::StackableImage;
use crate::ingest::ingest_run::IngestRun;
use crate::io::image::cfa::{CfaImage, CfaType};
use crate::io::image::error::ImageError;
use crate::io::image::image_dimensions::ImageDimensions;
use crate::io::image::image_metadata::ImageMetadata;
use crate::io::image::image_provenance::DemosaicProvenance;
use crate::io::image::linear::LinearImage;
use crate::io::image::load_context::LoadContext;
use crate::progress::progress_callback::ProgressCallback;
use crate::progress::stacking_progress::StackingStage;

fn load_drizzle_frame<I: StackableImage, P: AsRef<Path>>(
    frame: DrizzleFrame<P>,
    context: &LoadContext,
) -> Result<DrizzleFrame<I>, DrizzleError> {
    let DrizzleFrame {
        source,
        warp,
        pixel_weight_map,
    } = frame;
    let image = match I::load(source.as_ref(), context) {
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
/// already held in memory, use [`drizzle_images`]; to drizzle un-demosaiced sensor frames, use
/// [`drizzle_cfa_stack`].
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
    drizzle_paths::<LinearImage, P>(frames, drizzle, stack, &progress, cancel)
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
    drizzle_held(frames, drizzle, stack, &progress, cancel)
}

/// Drizzle calibrated, un-demosaiced sensor frames from disk — mosaic FITS or camera RAW — and
/// combine them under `stack`: CFA (Bayer or X-Trans) drizzle.
///
/// Each photosite deposits into the output channel of its colour alone, so a colour plane holds
/// only that colour's own samples, and an output pixel no photosite of a colour reached is left
/// out of that channel's combine. That takes many well-dithered frames to fill every colour at
/// every pixel; a few undithered frames are better demosaiced and stacked. Register the frames on
/// [`CfaImage::green_proxy`], which interpolates nothing but the green the stars are measured on.
///
/// A normalized combine compares each frame with its reference over the pixels both reached in a
/// colour. At scale 1 a sub-pixel dither shares them all; above it, two frames a photosite apart
/// can share none, and the combine then fails with [`StackError::NoCommonCoverage`].
/// `Normalization::None` takes any set.
///
/// A RAW file is decoded as it stands: the drizzle calibrates nothing. Calibrate the frames first,
/// and drizzle the calibrated mosaics, through [`drizzle_cfa_images`] or as FITS files.
///
/// # Errors
///
/// As [`drizzle_stack`], and for a frame whose mosaic pattern differs from the first frame's.
#[expect(
    clippy::needless_pass_by_value,
    reason = "every stacking entry takes its progress and cancel token the same way, by value"
)]
pub fn drizzle_cfa_stack<P: AsRef<Path>>(
    frames: Vec<DrizzleFrame<P>>,
    drizzle: &DrizzleConfig,
    stack: &StackConfig,
    progress: ProgressCallback,
    cancel: CancelToken,
) -> Result<DrizzleResult, DrizzleError> {
    drizzle_paths::<CfaImage, P>(frames, drizzle, stack, &progress, cancel)
}

/// Drizzle calibrated mosaics already held in memory, and combine them under `stack`: the
/// in-memory counterpart to [`drizzle_cfa_stack`]. The frames are consumed.
///
/// # Errors
///
/// As [`drizzle_cfa_stack`], but for the loads.
#[expect(
    clippy::needless_pass_by_value,
    reason = "every stacking entry takes its progress and cancel token the same way, by value"
)]
pub fn drizzle_cfa_images(
    frames: Vec<DrizzleFrame<CfaImage>>,
    drizzle: &DrizzleConfig,
    stack: &StackConfig,
    progress: ProgressCallback,
    cancel: CancelToken,
) -> Result<DrizzleResult, DrizzleError> {
    drizzle_held(frames, drizzle, stack, &progress, cancel)
}

/// The path entries' body: each frame decoded as an `I` when its turn comes.
fn drizzle_paths<I: StackableImage, P: AsRef<Path>>(
    frames: Vec<DrizzleFrame<P>>,
    drizzle: &DrizzleConfig,
    stack: &StackConfig,
    progress: &ProgressCallback,
    cancel: CancelToken,
) -> Result<DrizzleResult, DrizzleError> {
    let frame_count = frames.len();
    let run = IngestRun::new(&drizzle.ingest, cancel);
    let context = run.context.clone();
    // Lazy, so only the frame currently being drizzled is resident.
    let loaded = frames
        .into_iter()
        .map(move |frame| load_drizzle_frame::<I, P>(frame, &context));
    accumulate(
        loaded,
        frame_count,
        FrameOrigin::Files,
        drizzle,
        stack,
        &run,
        progress,
    )
}

/// The in-memory entries' body.
fn drizzle_held<I: StackableImage>(
    frames: Vec<DrizzleFrame<I>>,
    drizzle: &DrizzleConfig,
    stack: &StackConfig,
    progress: &ProgressCallback,
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
        progress,
    )
}

/// Drizzle every frame on its own, store it, combine the stored frames, and gate the product —
/// the body every entry point shares.
///
/// Frames arrive as a lazy fallible iterator so the path entries keep one input image resident: an
/// item is produced, drizzled, stored, and dropped before the next is loaded.
fn accumulate<I: StackableImage>(
    mut frames: impl Iterator<Item = Result<DrizzleFrame<I>, DrizzleError>>,
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
    let deposit = Deposit::of(&first.source);
    tracing::info!(
        ?origin,
        frame_count,
        input_width = input_dims.width(),
        input_height = input_dims.height(),
        channels = input_dims.channels(),
        mosaic = ?deposit.mosaic(),
        output_scale = drizzle.scale,
        pixfrac = drizzle.pixfrac,
        kernel = ?drizzle.kernel,
        "Starting drizzle stacking"
    );
    let output = ImageDimensions::new(
        drizzle.output_size(input_dims.size()),
        deposit.output_channels(),
    );
    let tier = DrizzleTier::for_run(input_dims, output, deposit, frame_count, origin, stack, run)?;

    let mut stored = Vec::with_capacity(frame_count);
    // Every frame's drops' weight at each output pixel, before any frame weight, in the planes of
    // their weights: how much signal landed there, which the fill gate reads.
    let mut depth: ArrayVec<Buffer2<f32>, 3> = (0..deposit.weight_planes())
        .map(|_| Buffer2::new_default(output.width(), output.height()))
        .collect();
    let mut unconverged_points = 0;
    let mut metadata = None;
    for (index, frame) in iter::once(Ok(first)).chain(frames).enumerate() {
        // Between frames, so a cancelled run stops before loading and drizzling the next one.
        if cancel.is_cancelled() {
            return Err(DrizzleError::Cancelled);
        }
        let frame = frame?;
        // Measured on the frame as it arrives, before its drops are spread: the drizzled pixels
        // average several, and would understate it. A mosaic's are measured per colour, which
        // its drizzle makes the channels.
        let source_stats = FrameStats::measure(&frame.source).into_drizzled();
        let mut accumulator = DrizzleAccumulator::new(input_dims, deposit, drizzle.clone(), index)?;
        accumulator.add_frame(&frame)?;
        unconverged_points += accumulator.unconverged_points();
        let mut frame_metadata = frame.source.metadata().clone().warped(
            |position| frame.warp.apply(accumulator.reference_point(position)),
            output.size(),
        );
        if let Deposit::Mosaic(cfa_type) = deposit {
            colour_planes(&mut frame_metadata, cfa_type);
        }
        drop(frame);
        let DrizzledPlanes {
            pixels,
            drops,
            flags,
        } = accumulator.into_planes();
        for (depth, drops) in depth.iter_mut().zip(&drops) {
            for (depth, &weight) in depth.pixels_mut().iter_mut().zip(drops.weight.pixels()) {
                *depth += weight;
            }
        }
        metadata.get_or_insert_with(|| frame_metadata.clone());
        let mut image =
            LinearImage::from_planar_channels(output, pixels.into_iter().map(Buffer2::into_vec));
        image.metadata = frame_metadata;
        image.flags = flags;
        stored.push(tier.store(image, FrameQuality::Drizzled { drops }, source_stats)?);
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
    // all" and "reached enough" are one comparison. Each weight plane against its own deepest: a
    // mosaic's green photosites outnumber its red and blue ones, and each colour plane is gated as
    // the image of its own photosites.
    let thresholds: ArrayVec<f32, 3> = depth
        .iter()
        .map(|depth| {
            let deepest = depth.pixels().iter().copied().fold(0.0f32, f32::max);
            (drizzle.min_weight_fraction * deepest).max(f32::MIN_POSITIVE)
        })
        .collect();
    let plane = |channel: usize| channel.min(depth.len() - 1);
    product.fill_where(
        |channel, index| depth[plane(channel)].pixels()[index] < thresholds[plane(channel)],
        drizzle.fill_value,
    );
    Ok(DrizzleResult {
        product,
        unconverged_points,
    })
}

/// `metadata`, of a `cfa_type` mosaic, as the metadata of the colour planes its drizzle makes:
/// sensor colour with nothing interpolated, and no quantization step, which the drops' sums mix.
const fn colour_planes(metadata: &mut ImageMetadata, cfa_type: CfaType) {
    if let Some(provenance) = &mut metadata.provenance {
        provenance.color = cfa_type.demosaiced_color();
        provenance.demosaic = DemosaicProvenance::CfaDrizzle;
    }
    metadata.quantization_sigma = None;
}
