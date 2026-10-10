//! What a frame has to satisfy before the combine will read it.
//!
//! Geometry first, then contents: every read below and in the combine slices a plane to
//! `pixel_count`, so a short plane has to be named here rather than reported as a slice index
//! panic. Each pass is chunked so cancellation is polled once per chunk instead of once per
//! sample — the index is only wanted on the error path, where recomputing it is free.

use common::CancelToken;

use crate::combine::CANCEL_POLL_CHUNK;
use crate::combine::cache::set_facts::SetFacts;
use crate::combine::error::StackError;
use crate::frame_store::frame_quality::{FramePlane, FrameQuality};
use crate::frame_store::stackable_image::StackableImage;
use crate::frame_store::stored_plane::StoredPlane;
use crate::io::cancelled::Cancelled;
use crate::io::image::image_dimensions::ImageDimensions;

use crate::frame_store::stored_frame::StoredFrame;

/// The checks one frame of a set must pass, reporting their failures against its `index` and
/// polling `cancel` once per chunk.
#[derive(Debug, Clone, Copy)]
pub(crate) struct FrameCheck<'a> {
    pub(crate) index: usize,
    pub(crate) cancel: &'a CancelToken,
}

impl FrameCheck<'_> {
    /// Every sample of a decoded image is finite.
    pub(crate) fn samples(self, image: &impl StackableImage) -> Result<(), StackError> {
        self.sample_channels(
            (0..image.dimensions().channels()).map(|channel| image.channel(channel)),
        )
    }

    /// A stored frame, in the one fixed order every cache constructor uses: its geometry against
    /// the cache's, its facts against the set's ([`SetFacts`]), then its samples and its quality
    /// planes. Geometry first because every later read slices a plane to `pixel_count`, and the
    /// facts before the samples because they are a comparison, while the samples are a pass over
    /// every pixel.
    pub(crate) fn stored(
        self,
        frame: &StoredFrame,
        dimensions: ImageDimensions,
        facts: &mut SetFacts,
    ) -> Result<(), StackError> {
        Cancelled::check(self.cancel)?;
        self.geometry(frame, dimensions)?;
        facts.admit(self.index, &frame.source_stats.facts)?;
        self.stored_samples(&frame.channels, dimensions.pixel_count())?;
        self.stored_quality(frame, dimensions)
    }

    /// The quality pair of a stored frame, if it carries one — see [`Self::quality_pair`].
    pub(crate) fn stored_quality(
        self,
        frame: &StoredFrame,
        dimensions: ImageDimensions,
    ) -> Result<(), StackError> {
        let pixel_count = dimensions.pixel_count();
        match &frame.quality {
            FrameQuality::Planes {
                coverage,
                confidence,
            } => self.quality_pair(
                FramePlane::Coverage,
                coverage.chunk(0, pixel_count),
                confidence.chunk(0, pixel_count),
            ),
            FrameQuality::Drizzled { drops } => drops.iter().try_for_each(|drops| {
                self.quality_pair(
                    FramePlane::DropWeight,
                    drops.weight.chunk(0, pixel_count),
                    drops.confidence.chunk(0, pixel_count),
                )
            }),
            FrameQuality::None | FrameQuality::Mask { .. } => Ok(()),
        }
    }

    /// Every sample of a stored frame's channel planes is finite.
    pub(crate) fn stored_samples(
        self,
        channels: &[StoredPlane],
        pixel_count: usize,
    ) -> Result<(), StackError> {
        self.sample_channels(channels.iter().map(|plane| plane.chunk(0, pixel_count)))
    }

    /// A frame-quality pair: the `support` plane, a warp's coverage or a drizzle's drop weight,
    /// and its confidence, each in its own range, and agreeing on where the frame has support.
    ///
    /// That agreement — `support == 0` exactly where `confidence == 0`, the invariant
    /// [`FrameQuality`] documents — is what lets the combine gate a sample on support and be sure
    /// of a positive confidence to divide its noise by, and what keeps `source_noise_variance`'s
    /// reciprocal finite. The warp and the drizzle produce planes that satisfy it; this is where
    /// caller-supplied and spilled ones are held to it.
    ///
    /// One walk over the pair rather than one per plane, so the pairing costs nothing beyond the
    /// range checks that were already reading both.
    fn quality_pair(
        self,
        kind: FramePlane,
        support: &[f32],
        confidence: &[f32],
    ) -> Result<(), StackError> {
        debug_assert_eq!(
            support.len(),
            confidence.len(),
            "frame quality planes are validated for geometry before their values"
        );
        let index = self.index;
        for (chunk, (support, confidence)) in support
            .chunks(CANCEL_POLL_CHUNK)
            .zip(confidence.chunks(CANCEL_POLL_CHUNK))
            .enumerate()
        {
            Cancelled::check(self.cancel)?;
            let pixel = |offset| chunk * CANCEL_POLL_CHUNK + offset;
            for (offset, (&support, &confidence)) in support.iter().zip(confidence).enumerate() {
                for (kind, value) in [(kind, support), (FramePlane::Confidence, confidence)] {
                    if !kind.accepts(value) {
                        return Err(StackError::InvalidWarpPlaneValue {
                            index,
                            plane: kind,
                            pixel: pixel(offset),
                            value,
                        });
                    }
                }
                if (support > 0.0) != (confidence > 0.0) {
                    return Err(StackError::FrameQualityPairMismatch {
                        index,
                        pixel: pixel(offset),
                        plane: kind,
                        support,
                        confidence,
                    });
                }
            }
        }
        Ok(())
    }

    /// A stored frame's shape against the geometry the cache was built for.
    ///
    /// A stored plane carries no width or height, so this compares plane counts and sample counts
    /// — enough to guarantee every `chunk(..)` below is in range.
    fn geometry(self, frame: &StoredFrame, dimensions: ImageDimensions) -> Result<(), StackError> {
        let index = self.index;
        if frame.channels.len() != dimensions.channels() {
            return Err(StackError::StoredFrameChannels {
                index,
                expected: dimensions.channels(),
                actual: frame.channels.len(),
            });
        }
        if let FrameQuality::Drizzled { drops } = &frame.quality {
            assert!(
                drops.len() == 1 || drops.len() == dimensions.channels(),
                "a drizzled frame's drops are shared or one per channel, not {} for {}",
                drops.len(),
                dimensions.channels()
            );
        }
        let expected = dimensions.pixel_count();
        let planes = frame
            .channels
            .iter()
            .map(|plane| (FramePlane::Channel, plane))
            .chain(frame.quality.present());
        for (kind, plane) in planes {
            if plane.samples() != expected {
                return Err(StackError::StoredFramePlaneSamples {
                    index,
                    plane: kind,
                    expected,
                    actual: plane.samples(),
                });
            }
        }
        Ok(())
    }

    /// Every sample of `channels` is finite. Chunked so cancellation is polled per chunk rather
    /// than per sample; the pixel index is only recomputed on the error path.
    fn sample_channels<'s>(
        self,
        channels: impl IntoIterator<Item = &'s [f32]>,
    ) -> Result<(), StackError> {
        for (channel, samples) in channels.into_iter().enumerate() {
            for (chunk, values) in samples.chunks(CANCEL_POLL_CHUNK).enumerate() {
                Cancelled::check(self.cancel)?;
                for (offset, value) in values.iter().copied().enumerate() {
                    if !value.is_finite() {
                        return Err(StackError::NonFiniteImageSample {
                            index: self.index,
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
}
