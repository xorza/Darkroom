//! One frame as the combine engine sees it.

use std::fmt::Debug;

use arrayvec::ArrayVec;
use imaginarium::Buffer2;

use crate::frame_store::cache_key::CacheKey;
use crate::frame_store::error::FrameStoreError;
use crate::frame_store::frame_quality::FrameQuality;
use crate::frame_store::frame_spill::{Carries, Committed, Decoded, FrameSpill, PlaneDigests};
use crate::frame_store::frame_stats::FrameStats;
use crate::frame_store::plane_store::PlaneStore;
use crate::frame_store::stackable_image::{ImageParts, StackableImage};
use crate::frame_store::stored_gain::StoredGain;
use crate::frame_store::stored_plane::StoredPlane;
use crate::frame_store::stratified_samples::StratifiedSamples;
use crate::io::image::cfa::CfaType;
use crate::io::image::image_dimensions::ImageDimensions;
use crate::io::image::image_metadata::ImageMetadata;
use crate::io::image::pixel_flags::{PixelFlags, QualityFlags};

/// One frame as the combine engine sees it: its channel planes, the per-pixel quality it carries
/// if a warp produced one or its source declared pixels with no measurement, its flags when its
/// quality is a mask of them or it carries any but `NO_DATA` (which quality planes already hold),
/// the gain a flat applied to its pixels, and the statistics measured on the source before any
/// interpolation.
#[derive(Debug)]
pub(crate) struct StoredFrame {
    pub(crate) channels: ArrayVec<StoredPlane, 3>,
    /// Each slot's [`StratifiedSamples`], gathered as a frame of no quality went to disk, where
    /// reading them from its planes would read every page; `None` for a frame in memory, or one
    /// whose quality makes normalization sample the pixels the frames share instead.
    pub(crate) samples: Option<ArrayVec<StoredPlane, 3>>,
    pub(crate) quality: FrameQuality<StoredPlane>,
    pub(crate) flags: Option<StoredPlane<u8>>,
    pub(crate) flat_gain: Option<StoredGain>,
    pub(crate) source_stats: FrameStats,
}

/// Whether a stored frame of `quality` keeps `flags`: a mask reads them, and otherwise they are
/// kept when they hold any flag but `NO_DATA`.
const fn keeps<P>(flags: &PixelFlags, quality: &FrameQuality<P>) -> bool {
    quality.mask().is_some() || flags.contains_other_than(QualityFlags::NO_DATA)
}

/// A kept frame read back from the decode cache: its planes mapped, and the shape and metadata it
/// was committed with.
#[derive(Debug)]
pub(crate) struct ReusedFrame {
    pub(crate) frame: StoredFrame,
    pub(crate) dimensions: ImageDimensions,
    pub(crate) metadata: ImageMetadata,
}

impl ReusedFrame {
    /// The frame as the image it was decoded to, copied out of its maps into RAM: for an entry
    /// that changes the frame in place, as calibration does.
    pub(crate) fn into_image<I: StackableImage>(self) -> I {
        let Self {
            frame,
            dimensions,
            metadata,
        } = self;
        let size = dimensions.size();
        let pixels = dimensions.pixel_count();
        let parts = ImageParts {
            planes: frame
                .channels
                .iter()
                .map(|plane| Buffer2::new(size.width, size.height, plane.chunk(0, pixels).to_vec()))
                .collect(),
            flags: frame
                .flags
                .as_ref()
                .map(|plane| PixelFlags::from_bytes(size, plane.chunk(0, pixels))),
        };
        I::from_parts(parts, frame.source_stats.facts.cfa_type, metadata)
    }
}

impl StoredFrame {
    pub(crate) fn from_memory(
        image: impl StackableImage,
        quality: FrameQuality<Buffer2<f32>>,
        source_stats: FrameStats,
    ) -> Self {
        let flat_gain = image.flat_gain().map(StoredGain::from_memory);
        let ImageParts { planes, flags } = image.into_parts();
        let flags = flags
            .filter(|flags| keeps(flags, &quality))
            .map(|flags| StoredPlane::Memory(flags.into_buffer()));
        Self {
            channels: planes.into_iter().map(StoredPlane::Memory).collect(),
            samples: None,
            quality: quality.map(StoredPlane::Memory),
            flags,
            flat_gain,
            source_stats,
        }
    }

    /// Write the frame's channels, quality planes and flags to `store` and memory-map them back.
    ///
    /// Borrows everything it writes: the caller keeps its buffers, which is what lets the warp
    /// stage hand the same ones to the next frame rather than allocating a set that has to be
    /// faulted in from scratch.
    pub(crate) fn spill(
        store: &impl PlaneStore,
        image: &impl StackableImage,
        quality: &FrameQuality<Buffer2<f32>>,
        source_stats: FrameStats,
    ) -> Result<Self, FrameStoreError> {
        let channels = (0..image.dimensions().channels())
            .map(|channel| store.store_channel(channel, image.channel(channel)))
            .collect::<Result<_, _>>()?;
        let samples = quality
            .is_none()
            .then(|| Self::spill_samples(store, image))
            .transpose()?;
        let quality =
            quality.try_map(|plane, buffer| store.store_quality(plane, buffer.pixels()))?;
        let flags = image
            .flags()
            .filter(|flags| keeps(flags, &quality))
            .map(|flags| store.store_flags(flags.bytes()))
            .transpose()?;
        let flat_gain = image
            .flat_gain()
            .map(|gain| StoredGain::spill(store, gain))
            .transpose()?;
        Ok(Self {
            channels,
            samples,
            quality,
            flags,
            flat_gain,
            source_stats,
        })
    }

    /// Each slot's [`StratifiedSamples`] of `image`, written to `store`.
    fn spill_samples(
        store: &impl PlaneStore,
        image: &impl StackableImage,
    ) -> Result<ArrayVec<StoredPlane, 3>, FrameStoreError> {
        let size = image.dimensions().size();
        let mosaic = image.cfa_type().filter(CfaType::is_mosaic);
        let slots = mosaic.map_or(image.dimensions().channels(), |cfa| cfa.num_colors());
        (0..slots)
            .map(|slot| {
                let plane = image.channel(if mosaic.is_some() { 0 } else { slot });
                let samples = StratifiedSamples::new(size, mosaic.as_ref(), slot).gather(plane);
                store.store_samples(slot, &samples)
            })
            .collect()
    }

    /// [`Self::spill`], then commit the files as a kept frame decoded under `key`, with the
    /// image's shape and metadata.
    pub(crate) fn cache(
        spill: &FrameSpill<'_>,
        key: CacheKey,
        image: &impl StackableImage,
        quality: &FrameQuality<Buffer2<f32>>,
        source_stats: FrameStats,
    ) -> Result<Self, FrameStoreError> {
        debug_assert!(
            quality.coverage().is_none(),
            "a kept frame is unwarped, and so carries no quality planes"
        );
        let frame = Self::spill(spill, image, quality, source_stats)?;
        let carries = match (quality.mask(), &frame.flags) {
            (Some(_), _) => Carries::NullMask,
            (None, Some(_)) => Carries::Flags,
            (None, None) => Carries::Channels,
        };
        let digests = PlaneDigests::of(&frame.channels, frame.flags.as_ref());
        spill.commit(
            key,
            Decoded {
                dimensions: image.dimensions(),
                carries,
                metadata: image.metadata().clone(),
            },
            digests,
            &frame.source_stats,
        )?;
        Ok(frame)
    }

    /// The frame [`Self::cache`] committed to `spill`'s files under `key`, mapped, with the shape
    /// and metadata it was committed with, its flat gain back in RAM from them; `None` when there
    /// is none whole to reuse — no commit for this source under this key, or a plane missing, of
    /// another size, or holding other bytes than it was committed with. The digests read every
    /// page of the planes, which the checks a reused frame passes read next anyway.
    pub(crate) fn reuse(
        spill: &FrameSpill<'_>,
        key: CacheKey,
    ) -> Result<Option<ReusedFrame>, FrameStoreError> {
        let Some(Committed {
            stats: source_stats,
            decoded,
            digests,
        }) = spill.committed(key)
        else {
            return Ok(None);
        };
        let Decoded {
            dimensions,
            carries,
            metadata,
        } = decoded;
        if !spill.channels_on_disk(dimensions)
            || (carries.flags() && !spill.flags_on_disk(dimensions))
        {
            return Ok(None);
        }
        let channels: ArrayVec<StoredPlane, 3> = (0..dimensions.channels())
            .map(|channel| StoredPlane::map(&spill.channel_path(channel)))
            .collect::<Result<_, _>>()?;
        let quality = match carries {
            Carries::NullMask => FrameQuality::Mask {
                excluded: QualityFlags::NO_DATA,
            },
            Carries::Channels | Carries::Flags => FrameQuality::None,
        };
        let flags = carries
            .flags()
            .then(|| StoredPlane::map(&spill.flags_path()))
            .transpose()?;
        if PlaneDigests::of(&channels, flags.as_ref()) != digests {
            return Ok(None);
        }
        let flat_gain = metadata.flat_gain.as_deref().map(StoredGain::from_memory);
        Ok(Some(ReusedFrame {
            frame: Self {
                channels,
                samples: None,
                quality,
                flags,
                flat_gain,
                source_stats,
            },
            dimensions,
            metadata,
        }))
    }
}
