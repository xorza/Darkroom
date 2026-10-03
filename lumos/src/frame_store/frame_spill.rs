//! [`FrameSpill`]: where a spilled frame's planes and sidecars live on disk, and what they are called.
//!
//! Every name the frame store writes comes from [`FrameSpill`], so the writer that produced a file
//! and a later run looking for it cannot disagree about where it is. The files themselves are not
//! tracked individually: they all sit inside a
//! [`SpillDirectory`](crate::frame_store::spill_directory::SpillDirectory), which decides
//! whether they outlive the run.

use std::fmt::Display;
use std::fs::{self, File};
use std::path::{Path, PathBuf};

use arrayvec::ArrayVec;
use common::SerdeFormat;
use common::file_utils;
use memmap2::Mmap;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use crate::frame_store::cache_key::{self, CacheKey, DecoderKind};
use crate::frame_store::error::FrameStoreError;
use crate::frame_store::frame_quality::FramePlane;
use crate::frame_store::frame_stats::FrameStats;
use crate::frame_store::stackable_image::StackableImage;
use crate::io::image::image_dimensions::ImageDimensions;

use crate::frame_store::stored_plane::StoredPlane;

/// The sidecar layout pin: the digest of a fixed [`Commit`] and [`FrameStats`] as bitcode, which
/// `sidecar_layout_is_pinned` checks. Bitcode is not self-describing, so a file written with
/// another layout decodes into plausible nonsense instead of failing. [`SIDECAR_FORMAT`] is derived
/// from this pin, so the change that moves the layout also changes the tag every sidecar carries.
pub(crate) const SIDECAR_PIN: &str = "26eb8398fbbba964";

/// The tag every sidecar carries.
const SIDECAR_FORMAT: u64 = cache_key::pins_fingerprint(&[SIDECAR_PIN]);

/// The files one frame occupies inside a spill directory: one plane per channel, the optional
/// frame-quality planes, the optional null mask, and for a kept frame its statistics and
/// [`Commit`].
#[derive(Debug, Clone)]
pub(crate) struct FrameSpill<'a> {
    directory: &'a Path,
    stem: String,
}

impl<'a> FrameSpill<'a> {
    /// The files of a frame spilled for this run only, under `stem`.
    pub(crate) fn new(directory: &'a Path, stem: &str) -> Self {
        Self {
            directory,
            stem: stem.to_owned(),
        }
    }

    /// The files a kept cache holds for `canonical_source` decoded by `decoder`: a hash of both, so
    /// distinct sources or decoders never share a file and the same pair always finds its own.
    pub(crate) fn cached(
        directory: &'a Path,
        canonical_source: &Path,
        decoder: DecoderKind,
    ) -> Self {
        let mut hasher = blake3::Hasher::new();
        hasher.update(b"lumos-frame-cache\0");
        hasher.update(&[decoder.tag()]);
        hasher.update(canonical_source.as_os_str().as_encoded_bytes());
        Self {
            directory,
            stem: hasher.finalize().to_hex().to_string(),
        }
    }

    pub(crate) fn channel_path(&self, channel: usize) -> PathBuf {
        self.file(format_args!("_c{channel}.bin"))
    }

    /// Path of a frame-quality plane: [`FramePlane::Coverage`] or [`FramePlane::Confidence`].
    pub(crate) fn quality_path(&self, plane: FramePlane) -> PathBuf {
        debug_assert_ne!(
            plane,
            FramePlane::Channel,
            "a channel is not a quality plane"
        );
        self.file(format_args!("_{plane}.bin"))
    }

    /// Path of the null mask's bit plane.
    pub(crate) fn nulls_path(&self) -> PathBuf {
        self.file("_nulls.bin")
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

    /// Whether both quality planes are on disk at the size `dimensions` implies.
    pub(crate) fn quality_on_disk(&self, dimensions: ImageDimensions) -> bool {
        [FramePlane::Coverage, FramePlane::Confidence]
            .into_iter()
            .all(|plane| plane_on_disk(&self.quality_path(plane), dimensions))
    }

    /// Write the channels of `image` and memory-map them back.
    ///
    /// The files are not tracked for removal here. Everything the frame store spills lives inside
    /// a [`SpillDirectory`](crate::frame_store::spill_directory::SpillDirectory), which
    /// removes the directory on drop unless `keep_cache` asked for it to survive — so a per-file
    /// drop guard would either duplicate that or, if it ignored the flag, delete what the user
    /// asked to keep.
    pub(crate) fn spill_channels(
        &self,
        image: &impl StackableImage,
    ) -> Result<ArrayVec<StoredPlane, 3>, FrameStoreError> {
        let mut planes = ArrayVec::new();
        for channel in 0..image.dimensions().channels() {
            let path = self.channel_path(channel);
            StoredPlane::write(&path, image.channel(channel))?;
            planes.push(StoredPlane::map(&path)?);
        }
        Ok(planes)
    }

    /// Record a kept frame whose planes are all written, decoded under `key`. The commit record
    /// goes last, so a run killed before it leaves a cache the next run rebuilds.
    pub(crate) fn commit(
        &self,
        key: CacheKey,
        carries_quality: bool,
        stats: &FrameStats,
    ) -> Result<(), FrameStoreError> {
        write_sidecar(&self.stats_path(), stats)?;
        write_sidecar(
            &self.commit_path(),
            &Commit {
                key,
                carries_quality,
            },
        )
    }

    /// What the frame committed here under `key` recorded; `None` for no commit record, one under
    /// another key, or statistics that do not decode or are not valid.
    pub(crate) fn committed(&self, key: CacheKey) -> Option<Committed> {
        let commit: Commit = read_sidecar(&self.commit_path())?;
        if commit.key != key {
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
        Some(Committed {
            stats,
            carries_quality: commit.carries_quality,
        })
    }
}

/// The record that commits a kept frame: the key its planes were decoded under, and whether it
/// wrote quality planes beside its channels.
///
/// The second half is what makes a frame whose two quality planes are both gone a cache to rebuild
/// rather than a frame with no nulls: reusing its channels without them would put the fill under
/// every null into the stack as data.
#[derive(Debug, Serialize, Deserialize)]
struct Commit {
    key: CacheKey,
    carries_quality: bool,
}

/// What [`FrameSpill::committed`] read back.
#[derive(Debug)]
pub(crate) struct Committed {
    pub(crate) stats: FrameStats,
    pub(crate) carries_quality: bool,
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

pub(crate) fn write_file(path: &Path, bytes: &[u8]) -> Result<(), FrameStoreError> {
    file_utils::publish_bytes(path, bytes, file_utils::PublicationMode::Cache).map_err(|source| {
        FrameStoreError::WriteFile {
            path: path.to_path_buf(),
            source,
        }
    })
}

/// Memory-map a spilled file.
pub(crate) fn map_file(path: &Path) -> Result<Mmap, FrameStoreError> {
    let file = File::open(path).map_err(|source| FrameStoreError::OpenFile {
        path: path.to_path_buf(),
        source,
    })?;
    // SAFETY: lumos writes every file it maps through an atomic publish and never rewrites one in
    // place; a file replaced by another process after the map is the documented mmap hazard.
    unsafe { Mmap::map(&file) }.map_err(|source| FrameStoreError::MemoryMap {
        path: path.to_path_buf(),
        source,
    })
}

/// A sidecar payload behind its layout tag.
#[derive(Debug, Serialize, Deserialize)]
struct Sidecar<T> {
    format: u64,
    value: T,
}

fn write_sidecar<T: Serialize>(path: &Path, value: &T) -> Result<(), FrameStoreError> {
    let sidecar = Sidecar {
        format: SIDECAR_FORMAT,
        value,
    };
    // Sidecars are plain scalars; a failure here would be a broken derive, not anything the
    // filesystem or the caller can cause.
    let bytes = common::serialize(&sidecar, SerdeFormat::Bitcode)
        .expect("a sidecar of plain scalars always serializes");
    write_file(path, &bytes)
}

/// Read a sidecar back, or `None` if it is absent, unreadable, or not this layout.
fn read_sidecar<T: DeserializeOwned>(path: &Path) -> Option<T> {
    let bytes = fs::read(path).ok()?;
    let sidecar: Sidecar<T> = common::deserialize(&bytes, SerdeFormat::Bitcode).ok()?;
    (sidecar.format == SIDECAR_FORMAT).then_some(sidecar.value)
}

#[cfg(test)]
mod tests {
    use std::fs;

    use common::{FileIdentity, SerdeFormat, TempDir};

    use crate::frame_store::cache_key::{CacheKey, DecoderKind};
    use crate::frame_store::error::FrameStoreError;
    use crate::frame_store::frame_facts::FrameFacts;
    use crate::frame_store::frame_spill::{
        Commit, FrameSpill, SIDECAR_FORMAT, SIDECAR_PIN, Sidecar,
    };
    use crate::frame_store::frame_stats::FrameStats;
    use crate::io::image::cfa::CfaType;
    use crate::io::image::image_provenance::RowOrder;
    use crate::io::image::sample_domain::{SampleDomain, ScaleOrigin};
    use crate::io::raw::demosaic::bayer::CfaPattern;
    use crate::math::statistics::MedianMad;

    fn stats(channels: &[(f32, f32)], quantization_sigma: Option<f32>) -> FrameStats {
        FrameStats {
            channels: channels
                .iter()
                .map(|&(median, mad)| MedianMad { median, mad })
                .collect(),
            quantization_sigma,
            facts: FrameFacts {
                domain: Some(SampleDomain {
                    scale: 65535.0,
                    origin: ScaleOrigin::Declared,
                    unit: Some("ADU".to_owned()),
                }),
                row_order: Some(RowOrder::BottomUp),
                cfa_type: Some(CfaType::Bayer(CfaPattern::Gbrg)),
            },
        }
    }

    /// A key spelled out field by field, so the pin below does not move with `DECODE_VERSION`.
    const fn key(decode_version: u64) -> CacheKey {
        CacheKey {
            source: FileIdentity {
                len: 4,
                mtime_ns: -3,
            },
            decoder: DecoderKind::Cfa,
            decode_version,
        }
    }

    /// Bitcode is not self-describing, so a sidecar from another layout would decode into plausible
    /// nonsense. A change of the layout fails this until the pin moves, and the pin is what the tag
    /// every sidecar carries is derived from.
    #[test]
    fn sidecar_layout_is_pinned() {
        let mut bytes = Vec::new();
        let commit = Commit {
            key: key(7),
            carries_quality: true,
        };
        common::serialize_into(&commit, SerdeFormat::Bitcode, &mut bytes).unwrap();
        let stats = stats(&[(0.5, 0.25), (0.75, 0.125)], Some(2e-5));
        common::serialize_into(&stats, SerdeFormat::Bitcode, &mut bytes).unwrap();
        assert_eq!(
            &blake3::hash(&bytes).to_hex()[..16],
            SIDECAR_PIN,
            "the sidecar layout moved: set SIDECAR_PIN to this digest, which retags every sidecar \
             so a kept cache in the old layout is rebuilt rather than misread"
        );
    }

    /// A commit reads back exactly as written, and only under its own key; a record that is
    /// missing, corrupt, in another layout, or holding a sigma that would poison every weight
    /// derived from it reads as nothing to reuse.
    #[test]
    fn committed_reads_back_only_a_valid_record_under_its_key() {
        let directory = TempDir::new("frame_spill_commit");
        let spill = FrameSpill::new(directory.path(), "frame");
        let key = key(7);
        for (stats, carries_quality) in [
            (stats(&[(42.5, 3.25)], Some(2e-5)), true),
            (
                stats(&[(100.0, 1.5), (200.0, 2.5), (300.0, 3.5)], None),
                false,
            ),
        ] {
            spill.commit(key, carries_quality, &stats).unwrap();
            let committed = spill.committed(key).unwrap();
            assert_eq!(committed.carries_quality, carries_quality);
            assert_eq!(committed.stats.channels, stats.channels);
            assert_eq!(committed.stats.quantization_sigma, stats.quantization_sigma);
            assert_eq!(committed.stats.facts, stats.facts);
        }
        assert!(spill.committed(self::key(8)).is_none(), "another key");

        let valid = stats(&[(42.5, 3.25)], None);
        for sigma in [f32::NAN, f32::INFINITY, 0.0, -1.0] {
            spill
                .commit(key, false, &stats(&[(42.5, 3.25)], Some(sigma)))
                .unwrap();
            assert!(spill.committed(key).is_none(), "a sigma of {sigma}");
        }

        spill.commit(key, false, &valid).unwrap();
        fs::write(spill.stats_path(), b"bad").unwrap();
        assert!(spill.committed(key).is_none(), "corrupt statistics");

        let stale = common::serialize(
            &Sidecar {
                format: SIDECAR_FORMAT ^ 1,
                value: &valid,
            },
            SerdeFormat::Bitcode,
        )
        .unwrap();
        fs::write(spill.stats_path(), stale).unwrap();
        assert!(spill.committed(key).is_none(), "another layout tag");

        spill.commit(key, false, &valid).unwrap();
        fs::remove_file(spill.commit_path()).unwrap();
        assert!(spill.committed(key).is_none(), "no commit record");

        let blocker = directory.join("not_a_directory");
        fs::write(&blocker, b"file").unwrap();
        let error = FrameSpill::new(&blocker, "frame")
            .commit(key, false, &valid)
            .unwrap_err();
        let expected = blocker.join("frame.stats");
        assert!(matches!(
            error,
            FrameStoreError::WriteFile { path, .. } if path == expected
        ));
    }
}
