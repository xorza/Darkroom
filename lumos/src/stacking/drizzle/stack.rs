use std::io::Error;
use std::path::Path;

use crate::io::astro_image::AstroImage;
use crate::stacking::drizzle::accumulator::{DrizzleAccumulator, DrizzleFrame};
use crate::stacking::drizzle::config::DrizzleConfig;
use crate::stacking::drizzle::error::DrizzleError;
use crate::stacking::product::StackProduct;
use crate::stacking::progress::{ProgressCallback, StackingStage, report_progress};

fn load_drizzle_frame<P: AsRef<Path>>(
    frame: DrizzleFrame<P>,
) -> Result<DrizzleFrame<AstroImage>, DrizzleError> {
    let DrizzleFrame {
        source,
        transform,
        weight,
        pixel_weight_map,
    } = frame;
    let path = source.as_ref();
    let image = AstroImage::from_file(path).map_err(|error| DrizzleError::ImageLoad {
        path: path.to_path_buf(),
        source: Error::other(error),
    })?;
    Ok(DrizzleFrame {
        source: image,
        transform,
        weight,
        pixel_weight_map,
    })
}

/// Drizzle stack images from disk with per-frame transforms.
///
/// Streams frames one at a time (only one input image is resident at a time).
/// To drizzle frames already held in memory, use [`drizzle_images`].
///
/// # Arguments
///
/// * `frames` - Paths bundled with their transform and optional quality weights
/// * `config` - Drizzle configuration
/// * `progress` - Progress callback
///
/// # Returns
///
/// The drizzled result: image plus coverage, weight (`Σwᵢ`), and variance (`Σwᵢ²/(Σwᵢ)²`) maps.
///
/// # Errors
///
/// Returns an error for invalid configuration, missing frames, image loading failures,
/// inconsistent image dimensions, or invalid frame weights.
pub fn drizzle_stack<P: AsRef<Path>>(
    frames: Vec<DrizzleFrame<P>>,
    config: &DrizzleConfig,
    progress: ProgressCallback,
) -> Result<StackProduct, DrizzleError> {
    if frames.is_empty() {
        return Err(DrizzleError::NoFrames);
    }
    config.validate()?;

    let frame_count = frames.len();
    let mut frames = frames.into_iter();
    let first = load_drizzle_frame(frames.next().unwrap())?;
    let input_dims = first.source.dimensions;

    tracing::info!(
        input_width = input_dims.size.x,
        input_height = input_dims.size.y,
        channels = input_dims.channels,
        output_scale = config.scale,
        pixfrac = config.pixfrac,
        kernel = ?config.kernel,
        frame_count,
        "Starting drizzle stacking (from paths)"
    );

    let mut accumulator = DrizzleAccumulator::new(input_dims, config.clone())?;
    accumulator.add_frame(first)?;
    report_progress(&progress, 1, frame_count, StackingStage::Processing);

    for (index, frame) in frames.enumerate() {
        accumulator.add_frame(load_drizzle_frame(frame)?)?;
        report_progress(&progress, index + 2, frame_count, StackingStage::Processing);
    }

    Ok(accumulator.finalize())
}

/// Drizzle stack frames already held in memory.
///
/// In-memory counterpart to [`drizzle_stack`]: skips the per-frame disk load when
/// the caller already owns the decoded frames. The frames are consumed.
///
/// # Errors
///
/// Returns an error for invalid configuration, missing frames, inconsistent image dimensions, or
/// invalid frame weights.
pub fn drizzle_images(
    frames: Vec<DrizzleFrame<AstroImage>>,
    config: &DrizzleConfig,
    progress: ProgressCallback,
) -> Result<StackProduct, DrizzleError> {
    if frames.is_empty() {
        return Err(DrizzleError::NoFrames);
    }
    config.validate()?;

    let frame_count = frames.len();
    let input_dims = frames[0].source.dimensions;

    tracing::info!(
        input_width = input_dims.size.x,
        input_height = input_dims.size.y,
        channels = input_dims.channels,
        output_scale = config.scale,
        pixfrac = config.pixfrac,
        kernel = ?config.kernel,
        frame_count,
        "Starting drizzle stacking (in memory)"
    );

    let mut accumulator = DrizzleAccumulator::new(input_dims, config.clone())?;
    for (index, frame) in frames.into_iter().enumerate() {
        accumulator.add_frame(frame)?;
        report_progress(&progress, index + 1, frame_count, StackingStage::Processing);
    }

    Ok(accumulator.finalize())
}
