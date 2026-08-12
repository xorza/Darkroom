//! What a frame has to satisfy before the combine will read it.
//!
//! Geometry first, then contents: every read below and in the combine slices a plane to
//! `pixel_count`, so a short plane has to be named here rather than reported as a slice index
//! panic. Each pass is chunked so cancellation is polled once per chunk instead of once per
//! sample — the index is only wanted on the error path, where recomputing it is free.

use common::CancelToken;

use crate::io::image::image_dimensions::ImageDimensions;
use crate::io::image::image_provenance::RowOrder;
use crate::io::image::sample_domain::SampleDomain;
use crate::stacking::combine::CANCEL_POLL_CHUNK;
use crate::stacking::combine::error::Error;
use crate::stacking::combine::error::check_cancel;
use crate::stacking::frame_store::frame_quality::FramePlane;
use crate::stacking::frame_store::stored_plane::StoredPlane;
use crate::stacking::frame_store::{StackableImage, StoredFrame};

fn validate_sample_channels<'a>(
    index: usize,
    channels: impl IntoIterator<Item = &'a [f32]>,
    cancel: &CancelToken,
) -> Result<(), Error> {
    for (channel, samples) in channels.into_iter().enumerate() {
        // Cancellation is polled per chunk by chunking the iteration, not by testing the pixel
        // index inside it — the divisor was a modulo on every sample of every plane of every
        // frame, and the index is only wanted on the error path, where recomputing it is free.
        for (chunk, values) in samples.chunks(CANCEL_POLL_CHUNK).enumerate() {
            check_cancel(cancel)?;
            for (offset, value) in values.iter().copied().enumerate() {
                if !value.is_finite() {
                    return Err(Error::NonFiniteImageSample {
                        index,
                        channel,
                        pixel: chunk * CANCEL_POLL_CHUNK + offset,
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

/// Check that every frame declaring a sample domain declares the same one.
///
/// A frame that declares none — synthesized rather than decoded, or a preview raster — is skipped
/// rather than treated as agreeing: there is nothing to compare, and rejecting on it would refuse
/// every in-memory fixture. The first frame that does declare one becomes the reference, so the
/// error names a concrete pair.
///
/// Scale and unit get a reference each rather than sharing one, because a frame can state a scale
/// and no unit — and [`SampleDomain::commensurate_with`] is deliberately blind across that gap, so
/// it is not transitive. Comparing everything against frame 0 alone would let a `Jy/beam` frame and
/// a `count/s` frame through whenever the frame that happened to come first stated no unit; here
/// the first frame to state a unit owns that half of the reference, and the error names whichever
/// frame the mismatch is actually with.
pub(crate) fn validate_sample_domains(frames: &[StoredFrame]) -> Result<(), Error> {
    let mut scale: Option<(usize, &SampleDomain)> = None;
    let mut unit: Option<(usize, &SampleDomain)> = None;
    for (index, frame) in frames.iter().enumerate() {
        let Some(domain) = frame.source_stats.domain.as_ref() else {
            continue;
        };
        match scale {
            None => scale = Some((index, domain)),
            Some((reference_index, expected)) if domain.scale != expected.scale => {
                return Err(Error::SampleDomainMismatch {
                    index,
                    actual: domain.clone(),
                    reference_index,
                    expected: expected.clone(),
                });
            }
            Some(_) => {}
        }
        if domain.unit.is_none() {
            continue;
        }
        match unit {
            None => unit = Some((index, domain)),
            Some((reference_index, expected)) if domain.unit != expected.unit => {
                return Err(Error::SampleDomainMismatch {
                    index,
                    actual: domain.clone(),
                    reference_index,
                    expected: expected.clone(),
                });
            }
            Some(_) => {}
        }
    }
    Ok(())
}

/// Check that every frame declaring a row order declares the same one.
///
/// Rows are decoded in the order the file stores them — Siril's rule that `ROWORDER` "shall not be
/// used to unflip the image data for stacking" — so two frames declaring different orders are
/// vertically mirrored views of the same sky. Averaging them is meaningless, and registration
/// cannot reconcile it either: triangle matching rejects a mirrored field by default, and a
/// similarity transform has no reflection to express it with. The failure otherwise surfaces as an
/// unexplained registration failure a long way from the frame that caused it.
///
/// A frame declaring none — synthesized rather than decoded — is skipped rather than treated as
/// agreeing, as everywhere else here.
pub(crate) fn validate_row_orders(frames: &[StoredFrame]) -> Result<(), Error> {
    let mut reference: Option<(usize, RowOrder)> = None;
    for (index, frame) in frames.iter().enumerate() {
        let Some(row_order) = frame.source_stats.row_order else {
            continue;
        };
        match reference {
            None => reference = Some((index, row_order)),
            Some((reference_index, expected)) if row_order != expected => {
                return Err(Error::RowOrderMismatch {
                    index,
                    actual: row_order,
                    reference_index,
                    expected,
                });
            }
            Some(_) => {}
        }
    }
    Ok(())
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

/// Check a frame's frame-quality pair: each plane's own range, and that the two agree on where the
/// frame has support.
///
/// That agreement — `coverage == 0` exactly where `confidence == 0`, the invariant
/// [`FrameQuality`](crate::stacking::frame_store::frame_quality::FrameQuality) documents — is what
/// lets the combine gate a sample on coverage and be sure of a positive confidence to weight it by,
/// and what keeps `source_noise_variance`'s reciprocal finite. The warp produces planes that
/// satisfy it; this is where caller-supplied and spilled ones are held to it.
///
/// One walk over the pair rather than one per plane, so the pairing costs nothing beyond the range
/// checks that were already reading both.
pub(crate) fn validate_frame_quality(
    index: usize,
    coverage: &[f32],
    confidence: &[f32],
    cancel: &CancelToken,
) -> Result<(), Error> {
    debug_assert_eq!(
        coverage.len(),
        confidence.len(),
        "frame quality planes are validated for geometry before their values"
    );
    // Chunked for the same reason as `validate_sample_channels`: one cancel poll per chunk
    // instead of a modulo per sample.
    for (chunk, (coverage, confidence)) in coverage
        .chunks(CANCEL_POLL_CHUNK)
        .zip(confidence.chunks(CANCEL_POLL_CHUNK))
        .enumerate()
    {
        check_cancel(cancel)?;
        let pixel = |offset| chunk * CANCEL_POLL_CHUNK + offset;
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
                return Err(Error::FrameQualityPairMismatch {
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
