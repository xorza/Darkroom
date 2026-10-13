use std::fs;
use std::path::Path;
use std::sync::Arc;

use common::{FileId, FileIdentity, SerdeFormat, TempDir};
use fits_well::image::SampleType;
use imaginarium::Buffer2;

use crate::frame::capture_conditions::CaptureConditions;
use crate::frame::frame_facts::FrameFacts;
use crate::frame::frame_stats::{FrameStats, SlotSpread};
use crate::frame::unverified_conditions::UnverifiedConditions;
use crate::frame_store::decode_cache::cache_key::{CacheKey, DecoderKind};
use crate::frame_store::decode_cache::frame_spill::{
    Carries, Commit, Decoded, FrameSpill, PlaneDigests, SIDECAR_PIN, encode_sidecar,
};
use crate::frame_store::error::FrameStoreError;
use crate::image::calibration_state::CalibrationState;
use crate::image::cfa::cfa_pattern::CfaPattern;
use crate::image::cfa::cfa_type::CfaType;
use crate::image::cfa::demosaic::xtrans::markesteijn::MarkesteijnPasses;
use crate::image::flat_gain::{FlatGain, GainGrid};
use crate::image::image_dimensions::ImageDimensions;
use crate::image::image_metadata::ImageMetadata;
use crate::image::image_provenance::{
    ColorProvenance, DecoderProvenance, DemosaicProvenance, ImageProvenance, RowOrder,
    SourceContainer, TransferProvenance,
};
use crate::image::sample_domain::{Pedestal, SampleDomain, ScaleOrigin};
use crate::io::fits::fits_checksum_provenance::{FitsChecksumProvenance, FitsChecksumState};
use crate::io::fits::fits_transfer_provenance::{FitsHduProvenance, FitsTransferProvenance};
use crate::math::noise::mosaic_noise::MosaicNoise;
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
    SerdeFormat::Bitcode
        .serialize_into(&commit, &mut bytes)
        .unwrap();
    let stats = stats(&[(0.5, 0.25), (0.75, 0.125)], Some(2e-5));
    SerdeFormat::Bitcode
        .serialize_into(&stats, &mut bytes)
        .unwrap();
    assert_eq!(
        &blake3::hash(&bytes).to_hex()[..16],
        SIDECAR_PIN,
        "the sidecar layout moved: set SIDECAR_PIN to this digest, which retags every sidecar \
         so a kept cache in the old layout is rebuilt rather than misread"
    );
}

/// A commit reads back exactly as written, and only for its own source under its own key; a
/// record that is missing, corrupt, damaged past its digest, in another layout, or holding a
/// sigma that would poison every weight derived from it reads as nothing to reuse.
#[test]
fn committed_reads_back_only_a_valid_record_under_its_key() {
    let directory = TempDir::new("frame_spill_commit");
    let spill = FrameSpill::named(directory.path(), "frame", key(7));
    for (stats, carries) in [
        (stats(&[(42.5, 3.25)], Some(2e-5)), Carries::NullMask),
        (
            stats(&[(100.0, 1.5), (200.0, 2.5), (300.0, 3.5)], None),
            Carries::Flags,
        ),
    ] {
        spill.commit(decoded(carries), digests(), &stats).unwrap();
        let committed = spill.committed().unwrap();
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
    let other_key = FrameSpill {
        key: key(8),
        ..spill.clone()
    };
    assert!(other_key.committed().is_none(), "another key");
    let clash = FrameSpill {
        source: Path::new("/data/other.fits"),
        ..spill.clone()
    };
    assert!(
        clash.committed().is_none(),
        "another source under the same name"
    );

    let valid = stats(&[(42.5, 3.25)], None);
    for sigma in [f32::NAN, f32::INFINITY, 0.0, -1.0] {
        spill
            .commit(
                decoded(Carries::Channels),
                digests(),
                &stats(&[(42.5, 3.25)], Some(sigma)),
            )
            .unwrap();
        assert!(spill.committed().is_none(), "a sigma of {sigma}");
    }

    spill
        .commit(decoded(Carries::Channels), digests(), &valid)
        .unwrap();
    fs::write(spill.stats_path(), b"bad").unwrap();
    assert!(spill.committed().is_none(), "corrupt statistics");

    let mut stale = encode_sidecar(&valid);
    stale[0] ^= 1;
    fs::write(spill.stats_path(), &stale).unwrap();
    assert!(spill.committed().is_none(), "another layout tag");

    // A damaged commit that would still decode reads as absent: one byte of its payload
    // flipped, and the file cut short.
    spill
        .commit(decoded(Carries::Channels), digests(), &valid)
        .unwrap();
    let commit = fs::read(spill.commit_path()).unwrap();
    let mut flipped = commit.clone();
    *flipped.last_mut().unwrap() ^= 0x10;
    fs::write(spill.commit_path(), &flipped).unwrap();
    assert!(spill.committed().is_none(), "a flipped byte");
    fs::write(spill.commit_path(), &commit[..commit.len() - 1]).unwrap();
    assert!(spill.committed().is_none(), "a short file");
    fs::write(spill.commit_path(), &commit).unwrap();
    assert!(spill.committed().is_some(), "the commit as written");

    spill
        .commit(decoded(Carries::Channels), digests(), &valid)
        .unwrap();
    fs::remove_file(spill.commit_path()).unwrap();
    assert!(spill.committed().is_none(), "no commit record");

    let blocker = directory.join("not_a_directory");
    fs::write(&blocker, b"file").unwrap();
    let error = FrameSpill::named(&blocker, "frame", key(7))
        .commit(decoded(Carries::Channels), digests(), &valid)
        .unwrap_err();
    let expected = blocker.join("frame.stats");
    assert!(matches!(
        error,
        FrameStoreError::WriteFile { path, .. } if path == expected
    ));
}
