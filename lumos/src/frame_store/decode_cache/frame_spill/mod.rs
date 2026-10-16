//! [`FrameSpill`]: where a spilled frame's planes and sidecars live on disk, and what they are
//! called.
//!
//! Every name the decode cache writes comes from [`FrameSpill`], so the writer that produced a file
//! and a later run looking for it cannot disagree about where it is. What one run alone needs has
//! no name at all: it goes to [`RunScratch`](crate::frame_store::run_scratch::RunScratch).

use std::fmt::Display;
use std::fs;
use std::path::{Path, PathBuf};

use bytemuck::Pod;
use common::SerdeFormat;
use common::file_utils;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use crate::decode::fnv1a;
use crate::frame::admitted::Admitted;
use crate::frame::frame_facts::FrameFacts;
use crate::frame::frame_stats::FrameStats;
use crate::frame_store::decode_cache::cache_key::CacheKey;
use crate::frame_store::decode_cache::kept_frame::KeptPlanes;
use crate::frame_store::error::FrameStoreError;
use crate::frame_store::stored_plane::StoredPlane;
use crate::image::channel_layout::ChannelLayout;
use crate::image::image_dimensions::ImageDimensions;
use crate::image::image_metadata::ImageMetadata;
use crate::math::size2us::Size2us;

/// The commit layout pin: the digest of a fixed [`Commit`] as bitcode, which
/// `sidecar_layout_is_pinned` checks. Bitcode is not self-describing, so a file written with
/// another layout decodes into plausible nonsense instead of failing. [`SIDECAR_FORMAT`] is derived
/// from this pin, so the change that moves the layout also changes the tag every commit carries.
const SIDECAR_PIN: &str = "52be17bb48c4c692";

/// The tag every commit carries.
const SIDECAR_FORMAT: u64 = fnv1a::fingerprint(&[SIDECAR_PIN]);

/// The files one kept frame occupies in the decode cache: one plane per channel, its flags, its
/// stratified samples and its flat gain's node planes when it has them, and its [`Commit`] —
/// under a name of the whole decode it holds.
#[derive(Debug, Clone)]
pub(crate) struct FrameSpill<'a> {
    directory: &'a Path,
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
    pub(crate) fn cached(directory: &'a Path, canonical_source: &'a Path, key: CacheKey) -> Self {
        let key_bytes = SerdeFormat::Bitcode
            .serialize(&key)
            .expect("a cache key of plain data always serializes");
        let hash = fnv1a::hash(fnv1a::OFFSET, b"lumos-frame-cache\0");
        let hash = fnv1a::hash(hash, &key_bytes);
        let hash = fnv1a::hash(hash, canonical_source.as_os_str().as_encoded_bytes());
        Self {
            directory,
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

    /// Path of one channel's flat gain nodes.
    pub(crate) fn gain_path(&self, channel: usize) -> PathBuf {
        self.file(format_args!("_g{channel}.bin"))
    }

    /// Path of the [`Commit`].
    fn commit_path(&self) -> PathBuf {
        self.file(".commit")
    }

    fn file(&self, suffix: impl Display) -> PathBuf {
        self.directory.join(format!("{}{suffix}", self.stem))
    }

    /// The plane at `path` mapped; `None` when it cannot be, which a reuse reads as a frame to
    /// rebuild.
    pub(crate) fn map<T: Pod>(path: &Path) -> Option<StoredPlane<T>> {
        StoredPlane::map(path).ok()
    }

    /// Record a kept frame whose planes are all written: decoded into `decoded`, with the digests
    /// of the planes as written and what its admission found, in one record. It goes last, so a run killed
    /// before it leaves a cache the next run rebuilds.
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
                width: decoded.dimensions.width(),
                height: decoded.dimensions.height(),
                channels: decoded.dimensions.channels(),
                carries: decoded.carries,
                gain_channels: decoded.gain_channels,
                digests,
                metadata: decoded.metadata.clone(),
                stats: admitted.stats.clone(),
                facts: admitted.facts.clone(),
            },
        )
    }

    /// What the frame committed here for this source under this key recorded; `None` for no commit
    /// record, one of another source or under another key, or statistics that are not valid.
    pub(crate) fn committed(&self) -> Option<Committed> {
        let commit: Commit = read_sidecar(&self.commit_path())?;
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
        let dimensions =
            ImageDimensions::checked(Size2us::new(commit.width, commit.height), layout).ok()?;
        Some(Committed {
            admitted: Admitted {
                stats: commit.stats,
                facts: commit.facts,
            },
            decoded: Decoded {
                dimensions,
                carries: commit.carries,
                gain_channels: commit.gain_channels,
                metadata: commit.metadata,
            },
            digests: commit.digests,
        })
    }
}

/// The record that commits a kept frame: the key its planes were decoded under, the source they
/// are of, the frame's shape, which planes it wrote beside its channels, what those planes held,
/// the decode's metadata, which a later run takes the frame back with, and what its admission found
/// of it: its statistics and its facts.
///
/// The carried planes are what make a frame whose flags are gone a cache to rebuild rather than a
/// frame with no nulls: reusing its channels without them would put the fill under every null, or
/// a saturated value, into the stack as data. The digests are what make a plane changed under its
/// name, or one a power loss left zeroed behind its commit, a cache to rebuild too. The metadata
/// holds no flat gain: its nodes are planes of their own, mapped on reuse.
#[derive(Debug, Serialize, Deserialize)]
struct Commit {
    key: CacheKey,
    /// The source's canonical path, as its platform encodes it.
    source: Vec<u8>,
    width: usize,
    height: usize,
    channels: usize,
    carries: Carries,
    gain_channels: usize,
    digests: PlaneDigests,
    metadata: ImageMetadata,
    stats: FrameStats,
    facts: FrameFacts,
}

/// What a decode left beside its planes, which a commit records: the frame's shape, the planes it
/// wrote beside its channels, the channels of its flat gain, none for a frame no flat divided, and
/// its metadata with no flat gain.
#[derive(Debug, Clone)]
pub(crate) struct Decoded {
    pub(crate) dimensions: ImageDimensions,
    pub(crate) carries: Carries,
    pub(crate) gain_channels: usize,
    pub(crate) metadata: ImageMetadata,
}

/// The blake3 digest of each plane a reuse reads: every channel, the flags plane when the frame
/// carries one, each slot's samples, and each channel's gain nodes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct PlaneDigests {
    pub(crate) channels: Vec<[u8; 32]>,
    pub(crate) flags: Option<[u8; 32]>,
    pub(crate) samples: Vec<[u8; 32]>,
    pub(crate) gain: Vec<[u8; 32]>,
}

impl PlaneDigests {
    /// The digests of `planes` as they hold now.
    pub(crate) fn of(planes: &KeptPlanes) -> Self {
        let digest = |plane: &StoredPlane| -> [u8; 32] { blake3::hash(plane.bytes()).into() };
        Self {
            channels: planes.channels.iter().map(digest).collect(),
            flags: planes
                .flags
                .as_ref()
                .map(|plane| blake3::hash(plane.bytes()).into()),
            samples: planes.samples.iter().flatten().map(digest).collect(),
            gain: planes.gain.iter().map(digest).collect(),
        }
    }
}

/// What a kept frame wrote beside its channels.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum Carries {
    /// Nothing: a frame with no flag set.
    Channels,
    /// Its flags plane, holding no `NO_DATA`.
    Flags,
    /// Its flags plane, whose `NO_DATA` flags are the frame's quality mask.
    NullMask,
}

impl Carries {
    /// Whether a flags plane is among the files.
    pub(crate) const fn flags(self) -> bool {
        matches!(self, Self::Flags | Self::NullMask)
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
    file_utils::publish_bytes(
        path,
        &encode_sidecar(value),
        file_utils::PublicationMode::Cache,
    )
    .map_err(|source| FrameStoreError::WriteFile {
        path: path.to_path_buf(),
        source,
    })
}

/// Read a sidecar back, or `None` if it is absent, unreadable, damaged, or not this layout.
fn read_sidecar<T: DeserializeOwned>(path: &Path) -> Option<T> {
    decode_sidecar(&fs::read(path).ok()?)
}

#[cfg(test)]
pub(crate) mod internals {
    use std::fs;
    use std::path::Path;

    use crate::frame_store::decode_cache::cache_key::CacheKey;
    use crate::frame_store::decode_cache::frame_spill::{
        Commit, FrameSpill, read_sidecar, write_sidecar,
    };

    impl<'a> FrameSpill<'a> {
        /// A kept frame's files under `stem` in `directory`, of a source with no path, decoded
        /// under `key`: [`Self::cached`]'s naming with a stem a test can read.
        pub(crate) fn named(directory: &'a Path, stem: &str, key: CacheKey) -> Self {
            Self {
                directory,
                stem: stem.to_owned(),
                source: Path::new(""),
                key,
            }
        }

        /// Commit the frame again with the digest of channel 0 as it is on disk now: a test that
        /// edits that plane on purpose, to see it read back, keeps it reusable.
        pub(crate) fn reseal(&self) {
            let mut commit: Commit = read_sidecar(&self.commit_path()).unwrap();
            commit.digests.channels[0] =
                blake3::hash(&fs::read(self.channel_path(0)).unwrap()).into();
            write_sidecar(&self.commit_path(), &commit).unwrap();
        }
    }
}

#[cfg(test)]
mod tests;
