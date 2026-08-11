//! What a frame has to satisfy before the combine will read it.
//!
//! Geometry first, then contents: every read below and in the combine slices a plane to
//! `pixel_count`, so a short plane has to be named here rather than reported as a slice index
//! panic. Each pass is chunked so cancellation is polled once per chunk instead of once per
//! sample — the index is only wanted on the error path, where recomputing it is free.

use common::CancelToken;

use crate::io::image::image_dimensions::ImageDimensions;
use crate::stacking::combine::error::Error;
use crate::stacking::combine::error::check_cancel;
use crate::stacking::frame_store::stored_plane::StoredPlane;
use crate::stacking::frame_store::warp_quality::FramePlane;
use crate::stacking::frame_store::{StackableImage, StoredFrame};

const VALIDATION_CHUNK_SIZE: usize = 16_384;

fn validate_sample_channels<'a>(
    index: usize,
    channels: impl IntoIterator<Item = &'a [f32]>,
    cancel: &CancelToken,
) -> Result<(), Error> {
    for (channel, samples) in channels.into_iter().enumerate() {
        // Cancellation is polled per chunk by chunking the iteration, not by testing the pixel
        // index inside it — the divisor was a modulo on every sample of every plane of every
        // frame, and the index is only wanted on the error path, where recomputing it is free.
        for (chunk, values) in samples.chunks(VALIDATION_CHUNK_SIZE).enumerate() {
            check_cancel(cancel)?;
            for (offset, value) in values.iter().copied().enumerate() {
                if !value.is_finite() {
                    return Err(Error::NonFiniteImageSample {
                        index,
                        channel,
                        pixel: chunk * VALIDATION_CHUNK_SIZE + offset,
                        value,
                    });
                }
            }
        }
    }
    Ok(())
}

pub(crate) fn validate_image_samples(
    image: &impl StackableImage,
    index: usize,
    cancel: &CancelToken,
) -> Result<(), Error> {
    validate_sample_channels(
        index,
        (0..image.dimensions().channels()).map(|channel| image.channel(channel)),
        cancel,
    )
}

/// Check a stored frame's shape against the geometry the cache was built for.
///
/// The counterpart to the dimension checks [`FrameCache::from_stack_frames`](super::FrameCache::from_stack_frames) makes on
/// caller-supplied images. A stored plane carries no width or height, so this compares plane
/// counts and sample counts instead — enough to guarantee every `chunk(..)` below is in range.
pub(crate) fn validate_stored_geometry(
    frame: &StoredFrame,
    dimensions: ImageDimensions,
    index: usize,
) -> Result<(), Error> {
    if frame.channels.len() != dimensions.channels() {
        return Err(Error::StoredFrameChannels {
            index,
            expected: dimensions.channels(),
            actual: frame.channels.len(),
        });
    }
    let expected = dimensions.pixel_count();
    let planes = frame
        .channels
        .iter()
        .map(|plane| (FramePlane::Channel, plane))
        .chain(frame.quality.present());
    for (kind, plane) in planes {
        if plane.samples() != expected {
            return Err(Error::StoredFramePlaneSamples {
                index,
                plane: kind,
                expected,
                actual: plane.samples(),
            });
        }
    }
    Ok(())
}

pub(crate) fn validate_stored_samples(
    channels: &[StoredPlane],
    pixel_count: usize,
    index: usize,
    cancel: &CancelToken,
) -> Result<(), Error> {
    validate_sample_channels(
        index,
        channels.iter().map(|plane| plane.chunk(0, pixel_count)),
        cancel,
    )
}

/// Check a frame's warp-quality pair: each plane's own range, and that the two agree on where the
/// frame has support.
///
/// That agreement — `coverage == 0` exactly where `confidence == 0`, the invariant
/// [`WarpQuality`](crate::stacking::frame_store::warp_quality::WarpQuality) documents — is what
/// lets the combine gate a sample on coverage and be sure of a positive confidence to weight it by,
/// and what keeps `source_noise_variance`'s reciprocal finite. The warp produces planes that
/// satisfy it; this is where caller-supplied and spilled ones are held to it.
///
/// One walk over the pair rather than one per plane, so the pairing costs nothing beyond the range
/// checks that were already reading both.
pub(crate) fn validate_warp_quality(
    index: usize,
    coverage: &[f32],
    confidence: &[f32],
    cancel: &CancelToken,
) -> Result<(), Error> {
    debug_assert_eq!(
        coverage.len(),
        confidence.len(),
        "warp quality planes are validated for geometry before their values"
    );
    // Chunked for the same reason as `validate_sample_channels`: one cancel poll per chunk
    // instead of a modulo per sample.
    for (chunk, (coverage, confidence)) in coverage
        .chunks(VALIDATION_CHUNK_SIZE)
        .zip(confidence.chunks(VALIDATION_CHUNK_SIZE))
        .enumerate()
    {
        check_cancel(cancel)?;
        let pixel = |offset| chunk * VALIDATION_CHUNK_SIZE + offset;
        for (offset, (&coverage, &confidence)) in coverage.iter().zip(confidence).enumerate() {
            for (kind, value) in [
                (FramePlane::Coverage, coverage),
                (FramePlane::Confidence, confidence),
            ] {
                if !kind.accepts(value) {
                    return Err(Error::InvalidWarpPlaneValue {
                        index,
                        plane: kind,
                        pixel: pixel(offset),
                        value,
                    });
                }
            }
            if (coverage > 0.0) != (confidence > 0.0) {
                return Err(Error::WarpQualityPairMismatch {
                    index,
                    pixel: pixel(offset),
                    coverage,
                    confidence,
                });
            }
        }
    }
    Ok(())
}
