//! [`FrameSpill`]: where a spilled frame's planes and sidecars live on disk, and what they are
//! called.
//!
//! Every name the decode cache writes comes from [`FrameSpill`], so the writer that produced a file
//! and a later run looking for it cannot disagree about where it is. What one run alone needs has
//! no name at all: it goes to [`RunScratch`](crate::frame_store::run_scratch::RunScratch).

use std::fmt::Display;
use std::fs;
use std::path::{Path, PathBuf};

use common::SerdeFormat;
use common::file_utils;
use imaginarium::Buffer2;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use crate::frame::frame_plane::FramePlane;
use crate::frame::frame_stats::FrameStats;
use crate::frame_store::decode_cache::cache_key::CacheKey;
use crate::frame_store::decode_cache::fnv1a;
use crate::frame_store::error::FrameStoreError;
use crate::frame_store::plane_store::PlaneStore;
use crate::frame_store::stored_plane::StoredPlane;
use crate::image::image_dimensions::ImageDimensions;
use crate::image::image_metadata::ImageMetadata;
use crate::math::size2us::Size2us;

/// The sidecar layout pin: the digest of a fixed [`Commit`] and [`FrameStats`] as bitcode, which
/// `sidecar_layout_is_pinned` checks. Bitcode is not self-describing, so a file written with
/// another layout decodes into plausible nonsense instead of failing. [`SIDECAR_FORMAT`] is derived
/// from this pin, so the change that moves the layout also changes the tag every sidecar carries.
const SIDECAR_PIN: &str = "55dc39978396bae9";

/// The tag every sidecar carries.
const SIDECAR_FORMAT: u64 = fnv1a::fingerprint(&[SIDECAR_PIN]);

/// The files one kept frame occupies in the decode cache: one plane per channel, the optional
/// frame-quality planes, the optional null mask, its statistics and its [`Commit`] — under the key
/// of the decode it holds.
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
    /// hash of the source and the key's decoder, so the same pair always finds its own. Two pairs
    /// that share a name are told apart by the commit, which records the source's path and the
    /// [`CacheKey`] and is checked before a byte is reused: a clash rebuilds the frame, it never
    /// reads another's.
    pub(crate) fn cached(directory: &'a Path, canonical_source: &'a Path, key: CacheKey) -> Self {
        let hash = fnv1a::hash(fnv1a::OFFSET, b"lumos-frame-cache\0");
        let hash = fnv1a::hash(hash, &[key.decoder.tag()]);
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

    /// Path of the null mask's bit plane.
    pub(crate) fn flags_path(&self) -> PathBuf {
        self.file("_flags.bin")
    }

    /// Path of the [`Commit`] sidecar.
    fn commit_path(&self) -> PathBuf {
        self.file(".commit")
    }

    fn stats_path(&self) -> PathBuf {
        self.file(".stats")
    }

    fn file(&self, suffix: impl Display) -> PathBuf {
        self.directory.join(format!("{}{suffix}", self.stem))
    }

    /// Whether every channel plane is on disk at the size `dimensions` implies.
    pub(crate) fn channels_on_disk(&self, dimensions: ImageDimensions) -> bool {
        (0..dimensions.channels())
            .all(|channel| plane_on_disk(&self.channel_path(channel), dimensions))
    }

    /// Whether the flag plane is on disk at the size `dimensions` implies: one byte per pixel.
    pub(crate) fn flags_on_disk(&self, dimensions: ImageDimensions) -> bool {
        let expected = dimensions.pixel_count() as u64;
        fs::metadata(self.flags_path()).is_ok_and(|metadata| metadata.len() == expected)
    }

    /// Record a kept frame whose planes are all written, decoded into `decoded`, with the digests
    /// of the planes as written and its statistics. The commit record goes last, so a run killed
    /// before it leaves a cache the next run rebuilds.
    pub(crate) fn commit(
        &self,
        decoded: Decoded,
        digests: PlaneDigests,
        stats: &FrameStats,
    ) -> Result<(), FrameStoreError> {
        write_sidecar(&self.stats_path(), stats)?;
        let Decoded {
            dimensions,
            carries,
            metadata,
        } = decoded;
        write_sidecar(
            &self.commit_path(),
            &Commit {
                key: self.key,
                source: self.source.as_os_str().as_encoded_bytes().to_vec(),
                width: dimensions.width(),
                height: dimensions.height(),
                channels: dimensions.channels(),
                carries,
                digests,
                metadata,
            },
        )
    }

    /// What the frame committed here for this source under this key recorded; `None` for no commit
    /// record, one of another source or under another key, or statistics that do not decode or are
    /// not valid.
    pub(crate) fn committed(&self) -> Option<Committed> {
        let commit: Commit = read_sidecar(&self.commit_path())?;
        if commit.key != self.key || commit.source != self.source.as_os_str().as_encoded_bytes() {
            return None;
        }
        let stats: FrameStats = read_sidecar(&self.stats_path())?;
        // A file can decode cleanly and still hold a sigma that would poison every weight derived
        // from it, so the value is checked rather than just the layout.
        if stats
            .quantization_sigma
            .is_some_and(|sigma| !sigma.is_finite() || sigma <= 0.0)
        {
            return None;
        }
        let dimensions =
            ImageDimensions::checked(Size2us::new(commit.width, commit.height), commit.channels)?;
        Some(Committed {
            stats,
            decoded: Decoded {
                dimensions,
                carries: commit.carries,
                metadata: commit.metadata,
            },
            digests: commit.digests,
        })
    }
}

impl PlaneStore for FrameSpill<'_> {
    fn store_channel(
        &self,
        channel: usize,
        pixels: &[f32],
    ) -> Result<StoredPlane, FrameStoreError> {
        let path = self.channel_path(channel);
        StoredPlane::write(&path, pixels)?;
        StoredPlane::map(&path)
    }

    fn store_quality(
        &self,
        _plane: FramePlane,
        _pixels: &[f32],
    ) -> Result<StoredPlane, FrameStoreError> {
        unreachable!("a kept frame is unwarped, and so carries no quality planes")
    }

    fn store_flags(&self, bytes: &[u8]) -> Result<StoredPlane<u8>, FrameStoreError> {
        let path = self.flags_path();
        StoredPlane::write(&path, bytes)?;
        StoredPlane::map(&path)
    }

    fn store_samples(&self, slot: usize, samples: &[f32]) -> Result<StoredPlane, FrameStoreError> {
        let path = self.file(format_args!("_s{slot}.bin"));
        StoredPlane::write(&path, samples)?;
        StoredPlane::map(&path)
    }

    /// Held in memory: a kept frame's flat gain travels whole in its commit's metadata, a sixteenth
    /// of a plane per channel, and is read back from there.
    fn store_gain(&self, _channel: usize, nodes: &[f32]) -> Result<StoredPlane, FrameStoreError> {
        Ok(StoredPlane::Memory(Buffer2::new(
            nodes.len(),
            1,
            nodes.to_vec(),
        )))
    }
}

/// The record that commits a kept frame: the key its planes were decoded under, the source they
/// are of, the frame's shape, which planes it wrote beside its channels, what those planes held,
/// and the decode's metadata, which a later run takes the frame back with.
///
/// The carried planes are what make a frame whose flags are gone a cache to rebuild rather than a
/// frame with no nulls: reusing its channels without them would put the fill under every null, or
/// a saturated value, into the stack as data. The digests are what make a plane changed under its
/// name, or one a power loss left zeroed behind its commit, a cache to rebuild too.
#[derive(Debug, Serialize, Deserialize)]
struct Commit {
    key: CacheKey,
    /// The source's canonical path, as its platform encodes it.
    source: Vec<u8>,
    width: usize,
    height: usize,
    channels: usize,
    carries: Carries,
    digests: PlaneDigests,
    metadata: ImageMetadata,
}

/// What a decode left beside its planes, which a commit records: the frame's shape, the planes it
/// wrote beside its channels, and its metadata.
#[derive(Debug, Clone)]
pub(crate) struct Decoded {
    pub(crate) dimensions: ImageDimensions,
    pub(crate) carries: Carries,
    pub(crate) metadata: ImageMetadata,
}

/// The blake3 digest of each plane a reuse reads: every channel, and the flags plane when the
/// frame carries one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct PlaneDigests {
    pub(crate) channels: Vec<[u8; 32]>,
    pub(crate) flags: Option<[u8; 32]>,
}

impl PlaneDigests {
    /// The digests of `channels` and `flags` as they hold now.
    pub(crate) fn of(channels: &[StoredPlane], flags: Option<&StoredPlane<u8>>) -> Self {
        Self {
            channels: channels
                .iter()
                .map(|plane| blake3::hash(plane.bytes()).into())
                .collect(),
            flags: flags.map(|plane| blake3::hash(plane.bytes()).into()),
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
    pub(crate) stats: FrameStats,
    pub(crate) decoded: Decoded,
    pub(crate) digests: PlaneDigests,
}

/// Whether a spilled plane is on disk holding exactly one image's worth of `f32`.
///
/// A plane of any other length is a stale cache from different geometry, not a reusable one — the
/// same rule for a channel and for a quality plane, since both are written one image-sized plane at
/// a time.
fn plane_on_disk(path: &Path, dimensions: ImageDimensions) -> bool {
    let expected = (dimensions.pixel_count() * size_of::<f32>()) as u64;
    fs::metadata(path).is_ok_and(|metadata| metadata.len() == expected)
}

/// The bytes before a sidecar's payload: its layout tag, little-endian, and the blake3 digest of the
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
        Commit, Decoded, FrameSpill, PlaneDigests, read_sidecar,
    };
    use crate::image::image_dimensions::ImageDimensions;
    use crate::math::size2us::Size2us;

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

        /// Commit the frame again with the digests of its planes as they are on disk now: a test
        /// that edits a plane on purpose, to see it read back, keeps it reusable.
        pub(crate) fn reseal(&self, channels: usize) {
            let commit: Commit = read_sidecar(&self.commit_path()).unwrap();
            let digest = |path: &Path| blake3::hash(&fs::read(path).unwrap()).into();
            let digests = PlaneDigests {
                channels: (0..channels)
                    .map(|channel| digest(&self.channel_path(channel)))
                    .collect(),
                flags: commit.carries.flags().then(|| digest(&self.flags_path())),
            };
            let stats = read_sidecar(&self.stats_path()).unwrap();
            let decoded = Decoded {
                dimensions: ImageDimensions::checked(
                    Size2us::new(commit.width, commit.height),
                    commit.channels,
                )
                .unwrap(),
                carries: commit.carries,
                metadata: commit.metadata,
            };
            self.commit(decoded, digests, &stats).unwrap();
        }
    }
}

#[cfg(test)]
mod tests;
