//! [`KeptFrame`]: a frame of the decode cache, as a keep wrote it or a reuse mapped it back.

use std::path::Path;

use arrayvec::ArrayVec;
use bytemuck::Pod;
use imaginarium::Buffer2;

use crate::frame::admitted::Admitted;
use crate::frame::frame_quality::FrameQuality;
use crate::frame_store::decode_cache::frame_spill::{
    Carries, Committed, Decoded, FrameSpill, PlaneDigests,
};
use crate::frame_store::error::FrameStoreError;
use crate::frame_store::stored_frame::StoredFrame;
use crate::frame_store::stored_gain::StoredGain;
use crate::frame_store::stored_image::StoredImage;
use crate::frame_store::stored_plane::{PublishedPlane, StoredPlane};
use crate::image::image_dimensions::ImageDimensions;
use crate::image::pixel_flags::PixelFlags;
use crate::image::quality_flags::QualityFlags;
use crate::image::stackable_image::{ImageParts, StackableImage};

/// A kept frame, its planes mapped: the same whether this run wrote it or a run before did, so a
/// first run and a reuse give the combine frames of one shape.
#[derive(Debug)]
pub(crate) struct KeptFrame {
    /// The channels and flags, with the shape and the metadata the frame was committed with; its
    /// flat gain is the frame's own, mapped.
    pub(crate) image: StoredImage,
    samples: Option<ArrayVec<StoredPlane, 3>>,
    carries: Carries,
    flat_gain: Option<StoredGain>,
    pub(crate) admitted: Admitted,
}

/// Every plane a kept frame writes: its channels, its flags when it keeps them, each slot's
/// stratified samples when it has no quality mask, and its flat gain's nodes.
#[derive(Debug)]
pub(crate) struct KeptPlanes {
    pub(crate) channels: ArrayVec<StoredPlane, 3>,
    pub(crate) flags: Option<StoredPlane<u8>>,
    pub(crate) samples: Option<ArrayVec<StoredPlane, 3>>,
    pub(crate) gain: ArrayVec<StoredPlane, 3>,
}

impl KeptFrame {
    /// The frame of `planes`, as `decoded` describes them, as its admission found it.
    fn new(planes: KeptPlanes, decoded: Decoded, admitted: Admitted) -> Self {
        let KeptPlanes {
            channels,
            flags,
            samples,
            gain,
        } = planes;
        let Decoded {
            dimensions,
            carries,
            gain_channels,
            metadata,
        } = decoded;
        debug_assert_eq!(gain.len(), gain_channels);
        Self {
            image: StoredImage {
                metadata,
                dimensions,
                channels,
                flags,
                flat_gain: None,
            },
            samples,
            carries,
            flat_gain: (!gain.is_empty()).then(|| StoredGain::Mapped {
                size: dimensions.size(),
                planes: gain,
            }),
            admitted,
        }
    }

    /// Write `image` as it decoded, unwarped, to `spill`'s files, each mapped from the handle that
    /// wrote it, then commit them with what its admission found.
    pub(crate) fn keep(
        spill: &FrameSpill<'_>,
        image: &impl StackableImage,
        admitted: Admitted,
    ) -> Result<Self, FrameStoreError> {
        let quality = FrameQuality::for_unwarped(image);
        let channels: ArrayVec<PublishedPlane, 3> = (0..image.dimensions().channels())
            .map(|channel| {
                StoredPlane::publish(&spill.channel_path(channel), image.channel(channel))
            })
            .collect::<Result<_, _>>()?;
        let flags = image
            .flags()
            .filter(|flags| quality.keeps(flags))
            .map(|flags| StoredPlane::publish(&spill.flags_path(), flags.bytes()))
            .transpose()?;
        let samples = quality
            .is_none()
            .then(|| {
                StoredFrame::store_samples(image, |slot, samples| {
                    StoredPlane::publish(&spill.samples_path(slot), samples)
                })
            })
            .transpose()?;
        let gain: ArrayVec<PublishedPlane, 3> = image
            .flat_gain()
            .into_iter()
            .flat_map(|gain| gain.planes().enumerate())
            .map(|(channel, nodes)| StoredPlane::publish(&spill.gain_path(channel), nodes))
            .collect::<Result<_, _>>()?;
        let carries = match (quality.mask(), &flags) {
            (Some(_), _) => Carries::NullMask,
            (None, Some(_)) => Carries::Flags,
            (None, None) => Carries::Channels,
        };
        let metadata = image.metadata().clone();
        let decoded = Decoded {
            dimensions: image.dimensions(),
            carries,
            gain_channels: gain.len(),
            metadata,
        };
        let digest = |published: &PublishedPlane| published.digest;
        let digests = PlaneDigests {
            channels: channels.iter().map(digest).collect(),
            flags: flags.as_ref().map(|published| published.digest),
            samples: samples.iter().flatten().map(digest).collect(),
            gain: gain.iter().map(digest).collect(),
        };
        let plane = |published: PublishedPlane| published.plane;
        let planes = KeptPlanes {
            channels: channels.into_iter().map(plane).collect(),
            flags: flags.map(|published| published.plane),
            samples: samples.map(|samples| samples.into_iter().map(plane).collect()),
            gain: gain.into_iter().map(plane).collect(),
        };
        spill.commit(&decoded, digests, &admitted)?;
        Ok(Self::new(planes, decoded, admitted))
    }

    /// The frame [`Self::keep`] committed to `spill`'s files as `committed` records it, mapped;
    /// `None` when there is none whole to reuse — a plane that does not map or holds other bytes
    /// than it was committed with. The digests read every page of the planes, which the checks a
    /// reused frame passes read next anyway.
    pub(crate) fn reuse(spill: &FrameSpill<'_>, committed: Committed) -> Option<Self> {
        let Committed {
            admitted,
            decoded,
            digests,
        } = committed;
        let channels = (0..decoded.dimensions.channels())
            .map(|channel| FrameSpill::map(&spill.channel_path(channel)))
            .collect::<Option<_>>()?;
        let flags = if decoded.carries.flags() {
            Some(FrameSpill::map(&spill.flags_path())?)
        } else {
            None
        };
        let samples = match decoded.carries {
            Carries::NullMask => None,
            Carries::Channels | Carries::Flags => Some(
                (0..digests.samples.len())
                    .map(|slot| FrameSpill::map(&spill.samples_path(slot)))
                    .collect::<Option<_>>()?,
            ),
        };
        let gain = (0..decoded.gain_channels)
            .map(|channel| FrameSpill::map(&spill.gain_path(channel)))
            .collect::<Option<_>>()?;
        let planes = KeptPlanes {
            channels,
            flags,
            samples,
            gain,
        };
        (PlaneDigests::of(&planes) == digests).then(|| Self::new(planes, decoded, admitted))
    }

    pub(crate) const fn dimensions(&self) -> ImageDimensions {
        self.image.dimensions
    }

    /// The frame as the combine reads it, its planes where they are mapped.
    pub(crate) fn into_frame(self) -> StoredFrame {
        let Self {
            image,
            samples,
            carries,
            flat_gain,
            admitted: Admitted { stats, facts },
        } = self;
        let quality = match carries {
            Carries::NullMask => FrameQuality::Mask {
                excluded: QualityFlags::NO_DATA,
            },
            Carries::Channels | Carries::Flags => FrameQuality::None,
        };
        StoredFrame {
            channels: image.channels,
            samples,
            quality,
            flags: image.flags,
            flat_gain,
            source_stats: stats,
            facts,
        }
    }

    /// The frame [`Self::keep`] committed to `spill`'s files as `committed` records it, as the
    /// image it was decoded to, in RAM, its flat gain with it: for an entry that changes the frame
    /// in place, as calibration does. `None` as [`Self::reuse`] finds none whole. Each channel and
    /// the flags are copied out of their maps, then hashed in RAM, so a page of their files is read
    /// once; the samples, which an image does not carry, are not read.
    pub(crate) fn reuse_image<I: StackableImage>(
        spill: &FrameSpill<'_>,
        committed: Committed,
    ) -> Option<I> {
        let Committed {
            admitted,
            decoded,
            digests,
        } = committed;
        let size = decoded.dimensions.size();
        if digests.channels.len() != decoded.dimensions.channels()
            || digests.gain.len() != decoded.gain_channels
        {
            return None;
        }
        let pixels = size.pixel_count();
        let planes = digests
            .channels
            .iter()
            .enumerate()
            .map(|(channel, digest)| {
                copied(&spill.channel_path(channel), digest, pixels)
                    .map(|samples| Buffer2::new(size.width, size.height, samples))
            })
            .collect::<Option<_>>()?;
        let flags = match (decoded.carries.flags(), digests.flags) {
            (true, Some(digest)) => PixelFlags::from_bytes(Buffer2::new(
                size.width,
                size.height,
                copied(&spill.flags_path(), &digest, pixels)?,
            )),
            (false, None) => None,
            (true, None) | (false, Some(_)) => return None,
        };
        let gain = (0..decoded.gain_channels)
            .map(|channel| FrameSpill::map(&spill.gain_path(channel)))
            .collect::<Option<ArrayVec<StoredPlane, 3>>>()?;
        let digest = |plane: &StoredPlane| -> [u8; 32] { blake3::hash(plane.bytes()).into() };
        if !gain.iter().map(digest).eq(digests.gain.iter().copied()) {
            return None;
        }
        let flat_gain =
            (!gain.is_empty()).then(|| StoredGain::Mapped { size, planes: gain }.to_flat_gain());
        Some(I::from_parts(
            ImageParts {
                planes,
                flags,
                flat_gain,
            },
            admitted.facts.cfa_type,
            decoded.metadata,
        ))
    }
}

/// The `len` samples of the plane at `path`, copied into RAM; `None` when it does not map, holds
/// another count, or its bytes are not `digest`'s.
fn copied<T: Pod>(path: &Path, digest: &[u8; 32], len: usize) -> Option<Vec<T>> {
    let plane = FrameSpill::map::<T>(path)?;
    let samples = plane.chunk(0..plane.samples()).to_vec();
    (samples.len() == len && blake3::hash(bytemuck::cast_slice(&samples)).as_bytes() == digest)
        .then_some(samples)
}
