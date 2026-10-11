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

use crate::frame_store::cache_key::{self, CacheKey, DecoderKind};
use crate::frame_store::error::FrameStoreError;
use crate::frame_store::frame_quality::FramePlane;
use crate::frame_store::frame_stats::FrameStats;
use crate::frame_store::plane_store::PlaneStore;
use crate::frame_store::stored_plane::StoredPlane;
use crate::io::image::image_dimensions::ImageDimensions;
use crate::io::image::image_metadata::ImageMetadata;
use crate::math::size2us::Size2us;

/// The sidecar layout pin: the digest of a fixed [`Commit`] and [`FrameStats`] as bitcode, which
/// `sidecar_layout_is_pinned` checks. Bitcode is not self-describing, so a file written with
/// another layout decodes into plausible nonsense instead of failing. [`SIDECAR_FORMAT`] is derived
/// from this pin, so the change that moves the layout also changes the tag every sidecar carries.
pub(crate) const SIDECAR_PIN: &str = "70df554b1dc47f3a";

/// The tag every sidecar carries.
const SIDECAR_FORMAT: u64 = cache_key::pins_fingerprint(&[SIDECAR_PIN]);

/// The files one kept frame occupies in the decode cache: one plane per channel, the optional
/// frame-quality planes, the optional null mask, its statistics and its [`Commit`].
#[derive(Debug, Clone)]
pub(crate) struct FrameSpill<'a> {
    directory: &'a Path,
    stem: String,
    /// The canonical path of the source the files are of, which the commit records.
    source: &'a Path,
}

impl<'a> FrameSpill<'a> {
    /// The files a kept cache holds for `canonical_source` decoded by `decoder`: named by an FNV-1a
    /// hash of both, so the same pair always finds its own. Two pairs that share a name are told
    /// apart by the commit, which records the source's path and the [`CacheKey`] and is checked
    /// before a byte is reused: a clash rebuilds the frame, it never reads another's.
    pub(crate) fn cached(
        directory: &'a Path,
        canonical_source: &'a Path,
        decoder: DecoderKind,
    ) -> Self {
        let hash = cache_key::fnv1a(cache_key::FNV1A_OFFSET, b"lumos-frame-cache\0");
        let hash = cache_key::fnv1a(hash, &[decoder.tag()]);
        let hash = cache_key::fnv1a(hash, canonical_source.as_os_str().as_encoded_bytes());
        Self {
            directory,
            stem: format!("{hash:016x}"),
            source: canonical_source,
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

    /// Record a kept frame whose planes are all written, decoded under `key` into `decoded`, with
    /// the digests of the planes as written. The commit record goes last, so a run killed before it
    /// leaves a cache the next run rebuilds.
    pub(crate) fn commit(
        &self,
        key: CacheKey,
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
                key,
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

    /// What the frame committed here for this source under `key` recorded; `None` for no commit
    /// record, one of another source or under another key, or statistics that do not decode or are
    /// not valid.
    pub(crate) fn committed(&self, key: CacheKey) -> Option<Committed> {
        let commit: Commit = read_sidecar(&self.commit_path())?;
        if commit.key != key || commit.source != self.source.as_os_str().as_encoded_bytes() {
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
    file_utils::publish_bytes(path, &bytes, file_utils::PublicationMode::Cache).map_err(|source| {
        FrameStoreError::WriteFile {
            path: path.to_path_buf(),
            source,
        }
    })
}

/// Read a sidecar back, or `None` if it is absent, unreadable, or not this layout.
fn read_sidecar<T: DeserializeOwned>(path: &Path) -> Option<T> {
    let bytes = fs::read(path).ok()?;
    let sidecar: Sidecar<T> = common::deserialize(&bytes, SerdeFormat::Bitcode).ok()?;
    (sidecar.format == SIDECAR_FORMAT).then_some(sidecar.value)
}

#[cfg(test)]
pub(crate) mod internals {
    use std::fs;
    use std::path::Path;

    use crate::frame_store::cache_key::CacheKey;
    use crate::frame_store::frame_spill::{
        Commit, Decoded, FrameSpill, PlaneDigests, read_sidecar,
    };
    use crate::io::image::image_dimensions::ImageDimensions;
    use crate::math::size2us::Size2us;

    impl<'a> FrameSpill<'a> {
        /// A kept frame's files under `stem` in `directory`, of a source with no path:
        /// [`Self::cached`]'s naming with a stem a test can read.
        pub(crate) fn named(directory: &'a Path, stem: &str) -> Self {
            Self {
                directory,
                stem: stem.to_owned(),
                source: Path::new(""),
            }
        }

        /// Commit the frame under `key` again with the digests of its planes as they are on disk
        /// now: a test that edits a plane on purpose, to see it read back, keeps it reusable.
        pub(crate) fn reseal(&self, key: CacheKey, channels: usize) {
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
            self.commit(key, decoded, digests, &stats).unwrap();
        }
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;

    use common::{FileId, FileIdentity, SerdeFormat, TempDir};

    use crate::frame_store::cache_key::{CacheKey, DecoderKind};
    use crate::frame_store::capture_conditions::CaptureConditions;
    use crate::frame_store::error::FrameStoreError;
    use crate::frame_store::frame_facts::FrameFacts;
    use std::sync::Arc;

    use fits_well::image::SampleType;
    use imaginarium::Buffer2;

    use crate::frame_store::frame_spill::{
        Carries, Commit, Decoded, FrameSpill, PlaneDigests, SIDECAR_FORMAT, SIDECAR_PIN, Sidecar,
    };
    use crate::frame_store::frame_stats::{FrameStats, SlotSpread};
    use crate::io::image::calibration_state::CalibrationState;
    use crate::io::image::cfa::CfaType;
    use crate::io::image::cfa::cfa_pattern::CfaPattern;
    use crate::io::image::fits::provenance::{
        FitsChecksumProvenance, FitsChecksumState, FitsHduProvenance, FitsTransferProvenance,
    };
    use crate::io::image::flat_gain::{FlatGain, GainGrid};
    use crate::io::image::image_dimensions::ImageDimensions;
    use crate::io::image::image_metadata::ImageMetadata;
    use crate::io::image::image_provenance::RowOrder;
    use crate::io::image::image_provenance::{
        ColorProvenance, DecoderProvenance, DemosaicProvenance, ImageProvenance, SourceContainer,
        TransferProvenance,
    };
    use crate::io::image::mosaic_noise::MosaicNoise;
    use crate::io::image::sample_domain::{Pedestal, SampleDomain, ScaleOrigin};
    use crate::io::image::unverified_conditions::UnverifiedConditions;
    use crate::io::raw::demosaic::xtrans::markesteijn::MarkesteijnPasses;
    use crate::math::size2us::Size2us;
    use crate::math::statistics::mad_to_sigma;

    fn stats(channels: &[(f32, f32)], quantization_sigma: Option<f32>) -> FrameStats {
        FrameStats {
            medians: channels.iter().map(|&(median, _)| median).collect(),
            noise: channels.iter().map(|&(_, mad)| mad_to_sigma(mad)).collect(),
            read_share: [0.0; 3].into_iter().collect(),
            sky: channels.iter().map(|&(median, _)| median).collect(),
            spread: Some(
                channels
                    .iter()
                    .map(|&(_, mad)| SlotSpread {
                        sigma: 2.0 * mad,
                        samples: 4096,
                    })
                    .collect(),
            ),
            quantization_sigma,
            electrons_per_unit: Some(2.5),
            facts: FrameFacts {
                domain: Some(SampleDomain {
                    scale: 65535.0,
                    origin: ScaleOrigin::Declared,
                    pedestal: Pedestal::Kept(2048.0),
                    unit: Some("ADU".to_owned()),
                }),
                row_order: Some(RowOrder::BottomUp),
                cfa_type: Some(CfaType::Bayer(CfaPattern::Gbrg)),
                saturation_flagged: true,
                conditions: CaptureConditions {
                    exposure_time: Some(300.0),
                    ccd_temp: Some(-10.0),
                    camera_temp: Some(21.5),
                },
                unverified_dark: UnverifiedConditions {
                    exposure: false,
                    temperature: true,
                },
            },
        }
    }

    fn digests() -> PlaneDigests {
        PlaneDigests {
            channels: vec![[1; 32]],
            flags: None,
        }
    }

    /// Metadata with every field set, its flat gain and every part of its provenance among them.
    fn metadata() -> ImageMetadata {
        let size = Size2us::new(9, 5);
        let grid = GainGrid::of(size);
        let nodes = (0..3)
            .map(|channel| {
                Buffer2::new(
                    grid.columns,
                    grid.rows,
                    (0..grid.columns * grid.rows)
                        .map(|node| 1.0 + 0.125 * (node + channel) as f32)
                        .collect(),
                )
            })
            .collect();
        ImageMetadata {
            object: Some("M 42".to_owned()),
            instrument: Some("ZWO ASI2600MC".to_owned()),
            telescope: Some("RedCat 51".to_owned()),
            date_obs: Some("2026-01-02T03:04:05".to_owned()),
            date_local: Some("2026-01-02T04:04:05".to_owned()),
            exposure_time: Some(300.0),
            iso: Some(800),
            sample_type: Some(SampleType::U16),
            camera_white_balance: Some([2.0, 1.0, 1.5, 1.0]),
            filter: Some("L-eXtreme".to_owned()),
            gain: Some(100.0),
            egain: Some(0.25),
            ccd_temp: Some(-10.0),
            camera_temp: Some(21.5),
            image_type: Some("LIGHT".to_owned()),
            xbinning: Some(1),
            ybinning: Some(2),
            set_temp: Some(-10.0),
            offset: Some(50),
            focal_length: Some(250.0),
            airmass: Some(1.25),
            ra_deg: Some(83.82),
            dec_deg: Some(-5.39),
            pixel_size_x: Some(3.76),
            pixel_size_y: Some(3.76),
            data_max: Some(65535.0),
            provenance: Some(ImageProvenance {
                container: SourceContainer::Fits,
                decoder: DecoderProvenance::FitsWell,
                transfer: TransferProvenance::FitsNormalized(FitsTransferProvenance {
                    bscale: 1.0,
                    bzero: 32768.0,
                    hdu: FitsHduProvenance {
                        index: 1,
                        extname: Some("SCI".to_owned()),
                        extver: Some(2),
                    },
                    checksum: FitsChecksumProvenance {
                        datasum: FitsChecksumState::Valid,
                        checksum: FitsChecksumState::Absent,
                    },
                }),
                color: ColorProvenance::SensorCfa,
                clipped: true,
                demosaic: DemosaicProvenance::LumosMarkesteijn {
                    passes: MarkesteijnPasses::Three,
                },
                row_order: RowOrder::BottomUp,
            }),
            domain: Some(SampleDomain {
                scale: 65535.0,
                origin: ScaleOrigin::Declared,
                pedestal: Pedestal::Kept(2048.0),
                unit: Some("ADU".to_owned()),
            }),
            quantization_sigma: Some(4.4e-6),
            mosaic_noise: Some(MosaicNoise {
                sigma: [0.01, 0.02, 0.03],
                read_share: [0.25, 0.5, 0.75],
                sky: [0.1, 0.2, 0.3],
                quantization_sigma: Some(4.4e-6),
            }),
            flat_gain: Some(Arc::new(FlatGain::from_nodes(size, nodes))),
            saturation_flagged: true,
            calibration: CalibrationState {
                bias: true,
                thermal: true,
                flat: false,
            },
            unverified_dark: UnverifiedConditions {
                exposure: true,
                temperature: false,
            },
        }
    }

    /// What a decode of 4 × 3 mono left, carrying `carries`, with [`metadata`].
    fn decoded(carries: Carries) -> Decoded {
        Decoded {
            dimensions: ImageDimensions::new((4, 3), 1),
            carries,
            metadata: metadata(),
        }
    }

    /// A key spelled out field by field, so the pin below does not move with `DECODE_VERSION` or
    /// the default options.
    const fn key(decode_version: u64) -> CacheKey {
        CacheKey {
            source: FileIdentity {
                len: 4,
                mtime_ns: -3,
                file_id: Some(FileId {
                    volume: 2049,
                    file: 131_077,
                }),
                changed_ns: Some(-2),
            },
            decoder: DecoderKind::Cfa,
            decode_version,
            options: 11,
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
            source: b"/data/lights/light_0001.cr2".to_vec(),
            width: 6024,
            height: 4022,
            channels: 1,
            carries: Carries::NullMask,
            digests: PlaneDigests {
                channels: vec![[3; 32], [5; 32]],
                flags: Some([7; 32]),
            },
            metadata: metadata(),
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

    /// A commit reads back exactly as written, and only for its own source under its own key; a
    /// record that is missing, corrupt, in another layout, or holding a sigma that would poison
    /// every weight derived from it reads as nothing to reuse.
    #[test]
    fn committed_reads_back_only_a_valid_record_under_its_key() {
        let directory = TempDir::new("frame_spill_commit");
        let spill = FrameSpill::named(directory.path(), "frame");
        let key = key(7);
        for (stats, carries) in [
            (stats(&[(42.5, 3.25)], Some(2e-5)), Carries::NullMask),
            (
                stats(&[(100.0, 1.5), (200.0, 2.5), (300.0, 3.5)], None),
                Carries::Flags,
            ),
        ] {
            spill
                .commit(key, decoded(carries), digests(), &stats)
                .unwrap();
            let committed = spill.committed(key).unwrap();
            assert_eq!(committed.decoded.carries, carries);
            assert_eq!(
                committed.decoded.dimensions,
                ImageDimensions::new((4, 3), 1)
            );
            assert_eq!(
                format!("{:?}", committed.decoded.metadata),
                format!("{:?}", metadata()),
                "every field of the metadata, its flat gain among them, reads back"
            );
            assert_eq!(committed.stats.medians, stats.medians);
            assert_eq!(committed.stats.quantization_sigma, stats.quantization_sigma);
            assert_eq!(committed.stats.facts, stats.facts);
        }
        assert!(spill.committed(self::key(8)).is_none(), "another key");
        let clash = FrameSpill {
            source: Path::new("/data/other.fits"),
            ..spill.clone()
        };
        assert!(
            clash.committed(key).is_none(),
            "another source under the same name"
        );

        let valid = stats(&[(42.5, 3.25)], None);
        for sigma in [f32::NAN, f32::INFINITY, 0.0, -1.0] {
            spill
                .commit(
                    key,
                    decoded(Carries::Channels),
                    digests(),
                    &stats(&[(42.5, 3.25)], Some(sigma)),
                )
                .unwrap();
            assert!(spill.committed(key).is_none(), "a sigma of {sigma}");
        }

        spill
            .commit(key, decoded(Carries::Channels), digests(), &valid)
            .unwrap();
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

        spill
            .commit(key, decoded(Carries::Channels), digests(), &valid)
            .unwrap();
        fs::remove_file(spill.commit_path()).unwrap();
        assert!(spill.committed(key).is_none(), "no commit record");

        let blocker = directory.join("not_a_directory");
        fs::write(&blocker, b"file").unwrap();
        let error = FrameSpill::named(&blocker, "frame")
            .commit(key, decoded(Carries::Channels), digests(), &valid)
            .unwrap_err();
        let expected = blocker.join("frame.stats");
        assert!(matches!(
            error,
            FrameStoreError::WriteFile { path, .. } if path == expected
        ));
    }
}
