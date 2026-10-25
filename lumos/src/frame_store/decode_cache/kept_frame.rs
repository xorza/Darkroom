//! [`KeptFrame`]: a frame of the decode cache, as a keep wrote it or a reuse mapped it back.

use std::mem;
use std::path::Path;
use std::sync::Arc;

use arrayvec::ArrayVec;
use bytemuck::Pod;
use common::Buffer2;

use crate::frame::admitted::Admitted;
use crate::frame::frame_quality::FrameQuality;
use crate::frame_store::decode_cache::frame_spill::{
    Committed, Decoded, FrameSpill, KeptGain, PlaneDigests,
};
use crate::frame_store::error::FrameStoreError;
use crate::frame_store::plane_sink::{PlaneRole, PlaneSink};
use crate::frame_store::stored_frame::{FrameWrite, SetKind, StoredFrame, WrittenFrame};
use crate::frame_store::stored_gain::StoredGain;
use crate::frame_store::stored_image::{StoredFlags, StoredImage};
use crate::frame_store::stored_plane::{PublishedPlane, StoredPlane};
use crate::image::flat_gain::frame_gain::FrameGain;
use crate::image::flat_gain::shared_gains::SharedGains;
use crate::image::flat_gain::{FlatGain, GainGrid};
use crate::image::image_dimensions::ImageDimensions;
use crate::image::pixel_flags::PixelFlags;
use crate::image::stackable_image::{ImageParts, StackableImage};
use common::Size2us;

/// A kept frame, its planes mapped: the same whether this run wrote it or a run before did, so a
/// first run and a reuse give the combine frames of one shape.
#[derive(Debug)]
pub(crate) struct KeptFrame {
    /// The channels, flags and flat gain, with the shape and the metadata the frame was committed
    /// with.
    pub(crate) image: StoredImage,
    samples: Option<ArrayVec<StoredPlane, 3>>,
    pub(crate) admitted: Admitted,
}

/// A kept frame read back into RAM, and its planes where the decode cache maps them.
#[derive(Debug)]
pub(crate) struct KeptImage<I> {
    pub(crate) image: I,
    pub(crate) stored: StoredImage,
}

/// A kept frame's files as a [`PlaneSink`]: each plane published under the name of its role, and
/// the digest of the bytes it wrote.
#[derive(Debug)]
struct KeptSink<'a> {
    spill: &'a FrameSpill<'a>,
    digests: PlaneDigests,
}

impl PlaneSink for KeptSink<'_> {
    fn write<T: Pod>(
        &mut self,
        role: PlaneRole,
        pixels: &[T],
    ) -> Result<StoredPlane<T>, FrameStoreError> {
        let spill = self.spill;
        let all = &mut self.digests;
        let (path, digests, idx) = match role {
            PlaneRole::Channel(channel) => {
                (spill.channel_path(channel), &mut all.channels, channel)
            }
            PlaneRole::Samples(slot) => (spill.samples_path(slot), &mut all.samples, slot),
            PlaneRole::Flags => {
                let PublishedPlane { plane, digest } =
                    StoredPlane::publish(&spill.flags_path(), pixels)?;
                all.flags = Some(digest);
                return Ok(plane);
            }
            PlaneRole::Quality => {
                unreachable!("a kept frame is unwarped, and carries no quality plane")
            }
            PlaneRole::Gain(_) => {
                unreachable!("a kept frame's gain is a source gain, which its key keeps once")
            }
        };
        let PublishedPlane { plane, digest } = StoredPlane::publish(&path, pixels)?;
        debug_assert_eq!(
            digests.len(),
            idx,
            "the planes of a role are written in order"
        );
        digests.push(digest);
        Ok(plane)
    }
}

impl KeptFrame {
    /// Write `image` as it decoded, unwarped, to `spill`'s files, each mapped from the handle that
    /// wrote it, then commit them with what its admission found.
    pub(crate) fn keep(
        spill: &FrameSpill<'_>,
        image: &impl StackableImage,
        admitted: Admitted,
    ) -> Result<Self, FrameStoreError> {
        let mut sink = KeptSink {
            spill,
            digests: PlaneDigests::default(),
        };
        let WrittenFrame {
            image: mut stored,
            samples,
            quality: _,
        } = FrameWrite {
            image,
            quality: FrameQuality::for_unwarped(image.flag_counts()),
            set: SetKind::Unwarped,
        }
        .write(&mut sink)?;
        debug_assert!(
            matches!(stored.flat_gain, None | Some(StoredGain::Shared(_))),
            "a decode's gain is a source gain, which its key keeps once"
        );
        let decoded = Decoded {
            dimensions: stored.dimensions,
            flags: stored.flag_counts().copied(),
            gain: image.flat_gain().map(|gain| KeptGain::of(gain)),
            metadata: mem::take(&mut stored.metadata),
        };
        spill.commit(&decoded, sink.digests, &admitted)?;
        stored.metadata = decoded.metadata;
        Ok(Self {
            image: stored,
            samples,
            admitted,
        })
    }

    /// The frame [`Self::keep`] committed to `spill`'s files as `committed` records it, mapped,
    /// each plane the commit lists, its flat gain the one `gains` holds of its key; `None` when
    /// there is none whole to reuse — a plane that does not map or holds other bytes than it was
    /// committed with. The digests read every page of the planes, which the checks a reused frame
    /// passes read next anyway.
    pub(crate) fn reuse(
        spill: &FrameSpill<'_>,
        committed: Committed,
        gains: &SharedGains,
    ) -> Option<Self> {
        let Committed {
            admitted,
            decoded:
                Decoded {
                    dimensions,
                    flags,
                    gain,
                    metadata,
                },
            digests,
        } = committed;
        let channels = (0..dimensions.channels())
            .map(|channel| FrameSpill::map(&spill.channel_path(channel)))
            .collect::<Option<_>>()?;
        let flags = match flags {
            Some(counts) => Some(StoredFlags {
                plane: FrameSpill::map(&spill.flags_path())?,
                counts,
            }),
            None => None,
        };
        let samples = if digests.samples.is_empty() {
            None
        } else {
            Some(
                (0..digests.samples.len())
                    .map(|slot| FrameSpill::map(&spill.samples_path(slot)))
                    .collect::<Option<_>>()?,
            )
        };
        let flat_gain = match gain {
            Some(gain) => Some(StoredGain::Shared(shared_gain(
                spill,
                gain,
                dimensions.size(),
                gains,
            )?)),
            None => None,
        };
        let image = StoredImage {
            metadata,
            dimensions,
            channels,
            flags,
            flat_gain,
        };
        (PlaneDigests::of(&image, samples.as_ref()) == digests).then_some(Self {
            image,
            samples,
            admitted,
        })
    }

    pub(crate) const fn dimensions(&self) -> ImageDimensions {
        self.image.dimensions
    }

    /// The frame as the combine reads it, its planes where they are mapped.
    pub(crate) fn into_frame(self) -> StoredFrame {
        let Self {
            image,
            samples,
            admitted,
        } = self;
        let quality = FrameQuality::for_unwarped(image.flag_counts());
        WrittenFrame {
            image,
            samples,
            quality,
        }
        .into_frame(admitted)
    }

    /// The frame [`Self::keep`] committed to `spill`'s files as `committed` records it, as the
    /// image it was decoded to, in RAM, its flat gain the one `gains` holds of its key, and its
    /// planes mapped where they lie: for an entry that changes the frame in place, as calibration
    /// does, or parks it. `None` as [`Self::reuse`] finds none whole. Each channel and the flags
    /// are copied out of their maps, then hashed in RAM, so a page of their files is read once;
    /// the samples, which an image does not carry, are not read.
    pub(crate) fn reuse_image<I: StackableImage>(
        spill: &FrameSpill<'_>,
        committed: Committed,
        gains: &SharedGains,
    ) -> Option<KeptImage<I>> {
        let Committed {
            admitted,
            decoded:
                Decoded {
                    dimensions,
                    flags,
                    gain,
                    metadata,
                },
            digests,
        } = committed;
        let size = dimensions.size();
        if digests.channels.len() != dimensions.channels() {
            return None;
        }
        let pixels = size.pixel_count();
        let mut planes = ArrayVec::new();
        let mut channels = ArrayVec::new();
        for (channel, digest) in digests.channels.iter().enumerate() {
            let Copied { plane, samples } = copied(&spill.channel_path(channel), digest, pixels)?;
            planes.push(Buffer2::new(size, samples));
            channels.push(plane);
        }
        let (image_flags, flags) = match (flags, digests.flags) {
            (Some(counts), Some(digest)) => {
                let Copied { plane, samples } = copied(&spill.flags_path(), &digest, pixels)?;
                (
                    PixelFlags::from_counted(Buffer2::new(size, samples), counts),
                    Some(StoredFlags { plane, counts }),
                )
            }
            (None, None) => (None, None),
            (Some(_), None) | (None, Some(_)) => return None,
        };
        let flat_gain = match gain {
            Some(gain) => Some(shared_gain(spill, gain, size, gains)?),
            None => None,
        };
        let image = I::from_parts(
            ImageParts {
                planes,
                flags: image_flags,
                flat_gain: flat_gain.clone().map(FrameGain::Shared),
            },
            admitted.facts.cfa_type,
            metadata.clone(),
        );
        Some(KeptImage {
            image,
            stored: StoredImage {
                metadata,
                dimensions,
                channels,
                flags,
                flat_gain: flat_gain.map(StoredGain::Shared),
            },
        })
    }
}

/// The flat gain `kept` names, of an image of `size`, as `gains` shares it: the one it holds of
/// the key, or the gain file's nodes, copied into RAM; `None` when the file does not map or holds
/// another gain than the key.
fn shared_gain(
    spill: &FrameSpill<'_>,
    kept: KeptGain,
    size: Size2us,
    gains: &SharedGains,
) -> Option<Arc<FlatGain>> {
    if let Some(shared) = gains.get(kept.key) {
        return Some(shared);
    }
    let grid = GainGrid::of(size);
    let count = grid.nodes();
    let file = FrameSpill::map::<f32>(&spill.gain_path(kept.key))?;
    if file.samples() != kept.channels * count {
        return None;
    }
    let gain = FlatGain::from_nodes(
        size,
        (0..kept.channels)
            .map(|channel| {
                let nodes = file.chunk(channel * count..(channel + 1) * count);
                Buffer2::new((grid.cols, grid.rows), nodes.to_vec())
            })
            .collect(),
    );
    (gain.key() == kept.key).then(|| gains.share(gain))
}

/// A plane's map, and its samples copied out of it into RAM.
#[derive(Debug)]
struct Copied<T> {
    plane: StoredPlane<T>,
    samples: Vec<T>,
}

/// The `len` samples of the plane at `path`, mapped and copied into RAM; `None` when it does not
/// map, holds another count, or its bytes are not `digest`'s.
fn copied<T: Pod>(path: &Path, digest: &[u8; 32], len: usize) -> Option<Copied<T>> {
    let plane = FrameSpill::map::<T>(path)?;
    let samples = plane.chunk(0..plane.samples()).to_vec();
    (samples.len() == len && blake3::hash(bytemuck::cast_slice(&samples)).as_bytes() == digest)
        .then_some(Copied { plane, samples })
}
