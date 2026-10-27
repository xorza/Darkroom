//! [`FrameSpill`]: where a spilled frame's planes and sidecars live on disk, and what they are
//! called.
//!
//! Every name the decode cache writes comes from [`FrameSpill`], so the writer that produced a file
//! and a later run looking for it cannot disagree about where it is. What one run alone needs has
//! no name at all: it goes to [`RunScratch`](crate::frame_store::run_scratch::RunScratch).

use std::borrow::Cow;
use std::fmt::Display;
use std::fs::File;
use std::io::Write;
use std::path::{Path, PathBuf};

use arrayvec::ArrayVec;
use bytemuck::Pod;
use common::{SerdeFormat, fs};
use imaginarium::{ChannelLayout, PlanarDesc};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use crate::frame::admitted::Admitted;
use crate::frame::frame_facts::FrameFacts;
use crate::frame::frame_stats::FrameStats;
use crate::frame_store::decode_cache::cache_key::CacheKey;
use crate::frame_store::error::FrameStoreError;
use crate::frame_store::stored_image::StoredImage;
use crate::frame_store::stored_plane::{PlaneDigest, StoredPlane};
use crate::image::flags::flag_counts::FlagCounts;
use crate::image::flat_gain::gain_key::GainKey;
use crate::image::flat_gain::{FlatGain, GainGrid};
use crate::image::frame_desc;
use crate::image::metadata::image_metadata::ImageMetadata;
use crate::io::decoder::fnv1a;
use common::Size2us;

/// The commit layout pin: the digest of a fixed [`Commit`] as bitcode, which
/// `sidecar_layout_is_pinned` checks. Bitcode is not self-describing, so a file written with
/// another layout decodes into plausible nonsense instead of failing. [`SIDECAR_FORMAT`] is derived
/// from this pin, so the change that moves the layout also changes the tag every commit carries.
const SIDECAR_PIN: &str = "63678a06bb280341";

/// The tag every commit carries.
const SIDECAR_FORMAT: u64 = fnv1a::fingerprint(&[SIDECAR_PIN.as_bytes()]);

/// The files one kept frame occupies in the decode cache: one plane per channel, its flags and its
/// stratified samples when it has them, and its [`Commit`] — under a name of the whole decode it
/// holds — and the file of its flat gain, under the gain's key, which every frame of that gain
/// shares.
#[derive(Debug, Clone)]
pub(crate) struct FrameSpill<'a> {
    dir: &'a Path,
    stem: String,
    /// The canonical path of the source the files are of, which the commit records.
    source: &'a Path,
    /// The decode the files hold, which the commit records and a reuse checks.
    key: CacheKey,
}

impl<'a> FrameSpill<'a> {
    /// The files a kept cache holds for `canonical_source` decoded under `key`: named by an FNV-1a
    /// hash of the source and the whole key — its file identity, decoder, decode version and
    /// options — so two decodes that would give other planes never share a name, and runs that
    /// write one name write the same planes. A clash of the hash is told apart by the commit,
    /// which records the source's path and the key and is checked before a byte is reused: a clash
    /// rebuilds the frame, it never reads another's.
    pub(crate) fn cached(dir: &'a Path, canonical_source: &'a Path, key: CacheKey) -> Self {
        let key_bytes = SerdeFormat::Bitcode
            .serialize(&key)
            .expect("a cache key of plain data always serializes");
        let hash = fnv1a::hash(fnv1a::OFFSET, b"lumos-frame-cache\0");
        let hash = fnv1a::hash(hash, &key_bytes);
        let hash = fnv1a::hash(hash, canonical_source.as_os_str().as_encoded_bytes());
        Self {
            dir,
            stem: format!("{hash:016x}"),
            source: canonical_source,
            key,
        }
    }

    pub(crate) fn channel_path(&self, channel: usize) -> PathBuf {
        self.file(format_args!("_c{channel}.bin"))
    }

    /// Path of the flags plane.
    pub(crate) fn flags_path(&self) -> PathBuf {
        self.file("_flags.bin")
    }

    /// Path of one slot's stratified samples.
    pub(crate) fn samples_path(&self, slot: usize) -> PathBuf {
        self.file(format_args!("_s{slot}.bin"))
    }

    /// Path of the flat gain of `key`: each channel's nodes in turn.
    pub(crate) fn gain_path(&self, key: GainKey) -> PathBuf {
        self.dir.join(format!("gain-{key}.bin"))
    }

    /// Write `gain`'s nodes to [`Self::gain_path`], published whole.
    pub(crate) fn publish_gain(&self, gain: &FlatGain) -> Result<(), FrameStoreError> {
        let path = self.gain_path(gain.key());
        fs::publish(&path, fs::PublicationMode::Cache, |file: &mut File| {
            gain.planes()
                .try_for_each(|nodes| file.write_all(bytemuck::cast_slice(nodes)))
        })
        .map_err(|source| FrameStoreError::WriteFile { path, source })
    }

    /// Path of the [`Commit`].
    fn commit_path(&self) -> PathBuf {
        self.file(".commit")
    }

    fn file(&self, suffix: impl Display) -> PathBuf {
        self.dir.join(format!("{}{suffix}", self.stem))
    }

    /// The plane at `path` mapped; `None` when it cannot be, which a reuse reads as a frame to
    /// rebuild.
    pub(crate) fn map<T: Pod>(path: &Path) -> Option<StoredPlane<T>> {
        StoredPlane::map(path).ok()
    }

    /// Record a kept frame whose planes are all written: decoded into `decoded`, with the digests
    /// of the planes as written and what its admission found, in one record. It goes last, so a run
    /// killed before it leaves a cache the next run rebuilds.
    pub(crate) fn commit(
        &self,
        decoded: &Decoded,
        digests: PlaneDigests,
        admitted: &Admitted,
    ) -> Result<(), FrameStoreError> {
        write_sidecar(
            &self.commit_path(),
            &Commit {
                key: self.key,
                source: self.source.as_os_str().as_encoded_bytes().to_vec(),
                width: decoded.desc.width(),
                height: decoded.desc.height(),
                channels: decoded.desc.channel_count(),
                flags: decoded.flags,
                gain: decoded.gain,
                digests,
                metadata: Cow::Borrowed(&decoded.metadata),
                stats: Cow::Borrowed(&admitted.stats),
                facts: Cow::Borrowed(&admitted.facts),
            },
        )
    }

    /// What the frame committed here for this source under this key recorded; `None` for no commit
    /// record, one of another source or under another key, or statistics that are not valid.
    pub(crate) fn committed(&self) -> Option<Committed> {
        let commit: Commit<'static> = read_sidecar(&self.commit_path())?;
        if commit.key != self.key || commit.source != self.source.as_os_str().as_encoded_bytes() {
            return None;
        }
        // A record can decode cleanly and still hold a sigma that would poison every weight derived
        // from it, so the value is checked rather than just the layout.
        if commit
            .stats
            .quantization_sigma
            .is_some_and(|sigma| !sigma.is_finite() || sigma <= 0.0)
        {
            return None;
        }
        let layout = ChannelLayout::try_from(commit.channels).ok()?;
        let desc = frame_desc::checked(Size2us::new(commit.width, commit.height), layout).ok()?;
        Some(Committed {
            admitted: Admitted {
                stats: commit.stats.into_owned(),
                facts: commit.facts.into_owned(),
            },
            decoded: Decoded {
                desc,
                flags: commit.flags,
                gain: commit.gain,
                metadata: commit.metadata.into_owned(),
            },
            digests: commit.digests,
        })
    }
}

/// The record that commits a kept frame: the key its planes were decoded under, the source they
/// are of, the frame's shape, how many pixels hold each flag of the flags it wrote, what its
/// planes held, the decode's metadata, which a later run takes the frame back with, and what its
/// admission found of it: its statistics and its facts.
///
/// The flag counts are what make a frame whose flags are gone a cache to rebuild rather than a
/// frame with no nulls: reusing its channels without them would put the fill under every null, or
/// a saturated value, into the stack as data. They also give a reuse the frame's quality, by the
/// rule its keep wrote it by, without a read of the plane. The digests are what make a plane
/// changed under its name, or one a power loss left zeroed behind its commit, a cache to rebuild
/// too. The metadata holds no flat gain: the commit names its file by its key, which a reuse checks
/// the file against.
///
/// A commit written borrows the decode's metadata and its admission's figures, which a `Cow`
/// serializes as the value itself; one read back owns them.
#[derive(Debug, Serialize, Deserialize)]
struct Commit<'a> {
    key: CacheKey,
    /// The source's canonical path, as its platform encodes it.
    #[serde(with = "common::hex_bytes")]
    source: Vec<u8>,
    width: usize,
    height: usize,
    channels: usize,
    flags: Option<FlagCounts>,
    gain: Option<KeptGain>,
    digests: PlaneDigests,
    metadata: Cow<'a, ImageMetadata>,
    stats: Cow<'a, FrameStats>,
    facts: Cow<'a, FrameFacts>,
}

/// What a decode left beside its planes, which a commit records: the frame's shape, the counts of
/// the flags it wrote, none for a frame with no flag set, its flat gain, none for a frame no flat
/// divided, and its metadata with no flat gain.
#[derive(Debug, Clone)]
pub(crate) struct Decoded {
    pub(crate) desc: PlanarDesc,
    pub(crate) flags: Option<FlagCounts>,
    pub(crate) gain: Option<KeptGain>,
    pub(crate) metadata: ImageMetadata,
}

/// A kept frame's flat gain: the key its file is named by and checked against, and its channels.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct KeptGain {
    pub(crate) key: GainKey,
    pub(crate) channels: usize,
}

impl KeptGain {
    /// The kept gain of `gain`.
    pub(crate) fn of(gain: &FlatGain) -> Self {
        Self {
            key: gain.key(),
            channels: gain.channels(),
        }
    }

    /// The bytes the gain holds in RAM over an image of `size`.
    pub(crate) const fn bytes(self, size: Size2us) -> usize {
        GainGrid::of(size).bytes(self.channels)
    }
}

/// The blake3 digest of each plane a reuse reads: every channel, the flags plane when the frame
/// carries one, and each slot's samples. The flat gain is checked against its key.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct PlaneDigests {
    pub(crate) channels: Vec<PlaneDigest>,
    pub(crate) flags: Option<PlaneDigest>,
    pub(crate) samples: Vec<PlaneDigest>,
}

impl PlaneDigests {
    /// The digests of `image`'s planes and of `samples`, as they hold now.
    pub(crate) fn of(image: &StoredImage, samples: Option<&ArrayVec<StoredPlane, 3>>) -> Self {
        let plane = |plane: &StoredPlane| PlaneDigest::of(plane.bytes());
        Self {
            channels: image.channels.iter().map(plane).collect(),
            flags: image
                .flags
                .as_ref()
                .map(|flags| PlaneDigest::of(flags.plane.bytes())),
            samples: samples.into_iter().flatten().map(plane).collect(),
        }
    }
}

/// What [`FrameSpill::committed`] read back.
#[derive(Debug)]
pub(crate) struct Committed {
    pub(crate) admitted: Admitted,
    pub(crate) decoded: Decoded,
    pub(crate) digests: PlaneDigests,
}

/// The bytes before a commit's payload: its layout tag, little-endian, and the blake3 digest of the
/// payload. The digest makes a file damaged anywhere — a bit flipped, a power loss that left it
/// short or zeroed — read as absent, where bitcode alone would decode the damage into plausible
/// values: a commit carries a frame's whole metadata, which no single check could vet.
const SIDECAR_HEADER: usize = size_of::<u64>() + blake3::OUT_LEN;

/// `value`'s sidecar bytes: the [`SIDECAR_HEADER`], then the value as bitcode.
fn encode_sidecar<T: Serialize>(value: &T) -> Vec<u8> {
    // Sidecars hold plain data; a failure here would be a broken derive, not anything the
    // filesystem or the caller can cause.
    let payload = SerdeFormat::Bitcode
        .serialize(value)
        .expect("a sidecar of plain data always serializes");
    let mut bytes = Vec::with_capacity(SIDECAR_HEADER + payload.len());
    bytes.extend_from_slice(&SIDECAR_FORMAT.to_le_bytes());
    bytes.extend_from_slice(blake3::hash(&payload).as_bytes());
    bytes.extend_from_slice(&payload);
    bytes
}

/// The value sidecar `bytes` hold; `None` when they are of another layout, damaged, or do not
/// decode.
fn decode_sidecar<T: DeserializeOwned>(bytes: &[u8]) -> Option<T> {
    let (format, rest) = bytes.split_first_chunk::<8>()?;
    let (digest, payload) = rest.split_first_chunk::<{ blake3::OUT_LEN }>()?;
    if u64::from_le_bytes(*format) != SIDECAR_FORMAT || blake3::hash(payload).as_bytes() != digest {
        return None;
    }
    SerdeFormat::Bitcode.deserialize(payload).ok()
}

fn write_sidecar<T: Serialize>(path: &Path, value: &T) -> Result<(), FrameStoreError> {
    fs::publish_bytes(path, &encode_sidecar(value), fs::PublicationMode::Cache).map_err(|source| {
        FrameStoreError::WriteFile {
            path: path.to_path_buf(),
            source,
        }
    })
}

/// Read a sidecar back, or `None` if it is absent, unreadable, damaged, or not this layout.
fn read_sidecar<T: DeserializeOwned>(path: &Path) -> Option<T> {
    decode_sidecar(&fs::read(path).ok()?)
}

#[cfg(test)]
pub(crate) mod internals {
    use std::path::Path;

    use common::fs;

    use crate::frame_store::decode_cache::cache_key::CacheKey;
    use crate::frame_store::decode_cache::frame_spill::{
        Commit, FrameSpill, read_sidecar, write_sidecar,
    };
    use crate::frame_store::stored_plane::PlaneDigest;

    impl<'a> FrameSpill<'a> {
        /// A kept frame's files under `stem` in `dir`, of a source with no path, decoded
        /// under `key`: [`Self::cached`]'s naming with a stem a test can read.
        pub(crate) fn named(dir: &'a Path, stem: &str, key: CacheKey) -> Self {
            Self {
                dir,
                stem: stem.to_owned(),
                source: Path::new(""),
                key,
            }
        }

        /// Commit the frame again with the digest of channel 0 as it is on disk now: a test that
        /// edits that plane on purpose, to see it read back, keeps it reusable.
        pub(crate) fn reseal(&self) {
            let mut commit: Commit<'static> = read_sidecar(&self.commit_path()).unwrap();
            commit.digests.channels[0] = PlaneDigest::of(&fs::read(self.channel_path(0)).unwrap());
            write_sidecar(&self.commit_path(), &commit).unwrap();
        }
    }
}

#[cfg(test)]
mod tests;
