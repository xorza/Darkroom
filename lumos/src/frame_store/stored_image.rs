//! A calibrated image stored between detection and registration.

use std::fmt::Debug;

use arrayvec::ArrayVec;

use crate::frame_store::error::FrameStoreError;
use crate::frame_store::frame_quality::FrameQuality;
use crate::frame_store::frame_stats::FrameStats;
use crate::frame_store::plane_store::PlaneStore;
use crate::frame_store::stored_frame::StoredFrame;
use crate::frame_store::stored_gain::StoredGain;
use crate::frame_store::stored_plane::StoredPlane;
use crate::io::image::image_dimensions::ImageDimensions;
use crate::io::image::image_metadata::ImageMetadata;
use crate::io::image::linear::LinearImage;
use crate::io::image::pixel_flags::{PixelFlags, QualityFlags};

/// A calibrated image stored on disk between detection and registration.
#[derive(Debug)]
pub(crate) struct StoredImage {
    pub(crate) metadata: ImageMetadata,
    pub(crate) dimensions: ImageDimensions,
    channels: ArrayVec<StoredPlane, 3>,
    /// The image's [`PixelFlags`] bytes, for an image that carries any. Spilled with the channels:
    /// without them the fill under every null reads back as a measurement.
    flags: Option<StoredPlane<u8>>,
}

impl StoredImage {
    /// Write `image`'s channels and flags to `store` and memory-map them back.
    pub(crate) fn spill(
        store: &impl PlaneStore,
        image: &LinearImage,
    ) -> Result<Self, FrameStoreError> {
        let flags = image
            .flags
            .as_ref()
            .map(|flags| store.store_flags(flags.bytes()))
            .transpose()?;
        let channels = (0..image.channels())
            .map(|channel| store.store_channel(channel, image.channel(channel).pixels()))
            .collect::<Result<_, _>>()?;
        Ok(Self {
            metadata: image.metadata.clone(),
            dimensions: image.dimensions(),
            channels,
            flags,
        })
    }

    /// Each channel's samples, read in place from the map.
    pub(crate) fn planes(&self) -> impl Iterator<Item = &[f32]> {
        let samples = self.dimensions.pixel_count();
        self.channels
            .iter()
            .map(move |plane| plane.chunk(0, samples))
    }

    /// The image's flags, copied out of the map: a byte a pixel, against the four a channel
    /// costs.
    pub(crate) fn flags(&self) -> Option<PixelFlags> {
        self.flags.as_ref().map(|plane| {
            PixelFlags::from_bytes(
                self.dimensions.size(),
                plane.chunk(0, self.dimensions.pixel_count()),
            )
        })
    }

    /// The image as the reference frame of a registered stack, its channels the planes already on
    /// disk: only the quality planes its flags imply are written, to `store` — see
    /// [`FrameQuality::for_reference`]. Its flags plane stays when it holds a flag the warp
    /// carries.
    pub(crate) fn into_reference_frame(
        self,
        store: &impl PlaneStore,
        source_stats: FrameStats,
    ) -> Result<StoredFrame, FrameStoreError> {
        let flags = self.flags();
        let quality = FrameQuality::for_reference(flags.as_ref())
            .try_map(|plane, buffer| store.store_quality(plane, buffer.pixels()))?;
        let carries = flags
            .as_ref()
            .is_some_and(|flags| flags.contains(QualityFlags::RESAMPLE_CARRIED));
        let flat_gain = self
            .metadata
            .flat_gain
            .as_deref()
            .map(|gain| StoredGain::spill(store, gain))
            .transpose()?;
        Ok(StoredFrame {
            channels: self.channels,
            quality,
            flags: self.flags.filter(|_| carries),
            flat_gain,
            source_stats,
        })
    }
}
