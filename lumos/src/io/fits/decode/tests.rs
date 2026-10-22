use common::{CancelToken, TempDir, fs};
use fits_well::FitsWriter;
use fits_well::header::Header;
use fits_well::image::{Bitpix, Compression, CompressionOptions, Image};
use fits_well::io::{BLOCK_SIZE, ChecksumReport, ChecksumStatus, HduKind};

use crate::image::cfa::cfa_pattern::CfaPattern;
use crate::image::channel_layout::ChannelLayout;
use crate::image::fits_checksum_provenance::FitsChecksumState;
use crate::image::fits_transfer_provenance::FitsTransferProvenance;
use crate::image::image_dimensions::ImageDimensions;
use crate::image::sample_domain::Pedestal;
use crate::internals::cfa::make_cfa;
use crate::internals::fits::{fits_refusal, fits_transfer, write_fits};
use crate::io::error::OverBudget;
use crate::io::fits::cfa;
use crate::io::fits::decode::plan::internals::HduSpec;
use crate::io::fits::decode::*;
use crate::io::fits::fits_load_config::{
    FitsChecksumPolicy, FitsCubeInterpretation, FitsFloatScale, FitsHduSelector, FitsLoadConfig,
};
use crate::math::size2us::Size2us;

fn load_ctx() -> LoadCtx {
    LoadCtx::new(CancelToken::never(), u64::MAX)
}

/// A cube read as RGB. Its float samples run to 400 with no `DATAMAX`, so they are said to be
/// normalized: these tests are about plane layout, not scale.
fn rgb_load_ctx() -> LoadCtx {
    LoadCtx {
        fits: FitsLoadConfig {
            cube: FitsCubeInterpretation::Rgb,
            float_scale: FitsFloatScale::Normalized,
            ..Default::default()
        },
        ..load_ctx()
    }
}

fn image_header(bitpix: i64, shape: &[usize]) -> Header {
    let mut header = Header::new();
    header.set("SIMPLE", true).unwrap();
    header.set("BITPIX", bitpix).unwrap();
    header.set("NAXIS", shape.len() as i64).unwrap();
    for (idx, &axis) in shape.iter().enumerate() {
        header
            .set(&format!("NAXIS{}", idx + 1), i64::try_from(axis).unwrap())
            .unwrap();
    }
    header
}

fn compressed_header(bitpix: i64, shape: &[usize]) -> Header {
    let mut header = Header::new();
    header.set("XTENSION", "BINTABLE").unwrap();
    header.set("BITPIX", 8).unwrap();
    header.set("NAXIS", 2).unwrap();
    header.set("NAXIS1", 8).unwrap();
    header.set("NAXIS2", 1).unwrap();
    header.set("PCOUNT", 0).unwrap();
    header.set("GCOUNT", 1).unwrap();
    header.set("ZIMAGE", true).unwrap();
    header.set("ZBITPIX", bitpix).unwrap();
    header.set("ZNAXIS", shape.len() as i64).unwrap();
    for (idx, &axis) in shape.iter().enumerate() {
        header
            .set(&format!("ZNAXIS{}", idx + 1), i64::try_from(axis).unwrap())
            .unwrap();
    }
    header
}

fn write_named_multi_image(path: &Path) {
    let mut writer = FitsWriter::new(fs::create(path).unwrap());
    let mut primary = Header::new();
    primary.set("SIMPLE", true).unwrap();
    primary.set("BITPIX", 8).unwrap();
    primary.set("NAXIS", 0).unwrap();
    primary.set("EXTEND", true).unwrap();
    writer.write_raw_hdu(&primary, &[]).unwrap();

    let mut first_header = Header::new();
    first_header.set("EXTNAME", "SCI").unwrap();
    first_header.set("EXTVER", 1).unwrap();
    writer
        .write_image(
            &Image::new([2, 1], vec![1.0f32, 2.0]).unwrap(),
            Some(&first_header),
        )
        .unwrap();

    let mut second_header = Header::new();
    second_header.set("EXTNAME", "SCI").unwrap();
    second_header.set("EXTVER", 2).unwrap();
    writer
        .write_image(
            &Image::new([2, 1, 3], vec![10.0f32, 20.0, 30.0, 40.0, 50.0, 60.0]).unwrap(),
            Some(&second_header),
        )
        .unwrap();
}

#[test]
fn shape_validation_rejects_zero_overflow_and_unsupported_cubes_without_panicking() {
    for shape in [&[0, 2][..], &[2, 0], &[2, 2, 0]] {
        let reason = fits_refusal(
            plan::dimensions_from_shape(shape, FitsCubeInterpretation::Reject).unwrap_err(),
        );
        assert_eq!(
            reason,
            FitsRefusal::ZeroAxis {
                shape: shape.to_vec()
            }
        );
    }

    // A side past the limit is refused before any count is formed from it; at the limit it is
    // accepted.
    let side = ImageDimensions::MAX_SIDE;
    for shape in [[side + 1, 2], [2, side + 1], [usize::MAX, 2]] {
        let reason = fits_refusal(
            plan::dimensions_from_shape(&shape, FitsCubeInterpretation::Reject).unwrap_err(),
        );
        assert_eq!(
            reason,
            FitsRefusal::SideTooLong {
                width: shape[0],
                height: shape[1]
            },
            "{shape:?}"
        );
    }
    assert_eq!(
        FitsRefusal::SideTooLong {
            width: side + 1,
            height: 2
        }
        .to_string(),
        "FITS image 1073741825x2 has a side past 1073741824 px"
    );
    assert_eq!(
        plan::dimensions_from_shape(&[side, 1], FitsCubeInterpretation::Reject)
            .unwrap()
            .size(),
        (side, 1).into()
    );

    let huge_shape = [1_000_000_000, 1_000_000_000, 4];
    let huge_cube = image_header(-32, &huge_shape);
    let reason = fits_refusal(
        HduSpec {
            header: &huge_cube,
            kind: HduKind::Primary,
            shape: &huge_shape,
            bitpix: Bitpix::F32,
            source_bytes: 0,
        }
        .description()
        .plan(
            FitsCubeInterpretation::Reject,
            FitsFloatScale::Auto,
            u64::MAX,
        )
        .unwrap_err(),
    );
    assert_eq!(reason, FitsRefusal::PlaneCount { planes: 4 });
    assert_eq!(reason.to_string(), "unsupported channel count (NAXIS3): 4");
}

#[test]
fn preflight_enforces_source_output_and_peak_limits_at_exact_boundaries() {
    let rgb_shape = [100, 100, 3];
    let rgb = image_header(-64, &rgb_shape);
    let plan = HduSpec {
        header: &rgb,
        kind: HduKind::Primary,
        shape: &rgb_shape,
        bitpix: Bitpix::F64,
        source_bytes: 241_920,
    }
    .description()
    .plan(FitsCubeInterpretation::Rgb, FitsFloatScale::Auto, u64::MAX)
    .unwrap();
    assert_eq!(plan.source_bytes, 241_920);
    assert_eq!(plan.decoded_bytes, 120_000);
    // The 120 000 decoded bytes, two stored chunks of the whole 100 rows at 8 bytes a sample, and
    // the flag plane a float image may need for its NaNs, a byte for each of the 10 000 pixels.
    assert_eq!(plan.peak_bytes, 290_000);
    // Beside the frame: the two chunks of 80 000 B. An extension read after the image holds the
    // reader's buffer, grown to the larger of the chunk and the extension, and its own copy: a
    // 10 000 B one 90 000, under the chunks; a 200 000 B one 400 000; a 100 000 B one 200 000,
    // and a 30 000 B one after it 130 000, the buffer still 100 000.
    for (extensions, beside) in [
        (&[][..], 160_000),
        (&[10_000][..], 160_000),
        (&[200_000][..], 400_000),
        (&[100_000, 30_000][..], 200_000),
    ] {
        assert_eq!(
            plan.beside_frame_bytes(extensions),
            beside,
            "{extensions:?}"
        );
    }
    assert_eq!(plan.rows_per_chunk, 100);
    HduSpec {
        header: &rgb,
        kind: HduKind::Primary,
        shape: &rgb_shape,
        bitpix: Bitpix::F64,
        source_bytes: 241_920,
    }
    .description()
    .plan(
        FitsCubeInterpretation::Rgb,
        FitsFloatScale::Auto,
        plan.peak_bytes,
    )
    .unwrap();
    let reason = fits_refusal(
        HduSpec {
            header: &rgb,
            kind: HduKind::Primary,
            shape: &rgb_shape,
            bitpix: Bitpix::F64,
            source_bytes: 241_920,
        }
        .description()
        .plan(
            FitsCubeInterpretation::Rgb,
            FitsFloatScale::Auto,
            plan.peak_bytes - 1,
        )
        .unwrap_err(),
    );
    assert_eq!(
        reason,
        FitsRefusal::OverBudget {
            allocation: FitsAllocation::PeakMemory,
            budget: OverBudget {
                required: 290_000,
                limit: 289_999
            }
        }
    );
    assert_eq!(
        reason.to_string(),
        "estimated peak memory needs 290000 bytes, past the load's memory limit of 289999 bytes"
    );
    // A flags extension is held beside the decoded 120 000 bytes as read, and as the decode's own
    // flag plane: 2 bytes for each of the 10 000 pixels, 140 000 in all.
    plan.admit_flags_extension(140_000).unwrap();
    let reason = fits_refusal(plan.admit_flags_extension(139_999).unwrap_err());
    assert_eq!(
        reason,
        FitsRefusal::OverBudget {
            allocation: FitsAllocation::DecodedOutputWithFlags,
            budget: OverBudget {
                required: 140_000,
                limit: 139_999
            }
        }
    );

    // An integer image with no BLANK holds no null, so it has no flag plane until a DATAMAX gives
    // the decode a level to flag against: 40 000 decoded bytes of f32 and two chunks of 100 rows
    // at 2 bytes a sample, 80 000, then 10 000 more. Without the DATAMAX a flags extension adds
    // its own 10 000 bytes to the decoded 40 000 and no decode plane: 50 000.
    let mono_shape = [100, 100];
    let mut mono = image_header(16, &mono_shape);
    let plan_of = |header: &Header| {
        HduSpec {
            header,
            kind: HduKind::Primary,
            shape: &mono_shape,
            bitpix: Bitpix::I16,
            source_bytes: 20_160,
        }
        .description()
        .plan(
            FitsCubeInterpretation::Reject,
            FitsFloatScale::Auto,
            u64::MAX,
        )
        .unwrap()
    };
    let without = plan_of(&mono);
    assert_eq!(without.peak_bytes, 80_000);
    without.admit_flags_extension(50_000).unwrap();
    assert!(without.admit_flags_extension(49_999).is_err());
    mono.set("DATAMAX", 30_000.0).unwrap();
    assert_eq!(plan_of(&mono).peak_bytes, 90_000);

    let compressed_shape = [1024, 1024];
    let compressed = compressed_header(-32, &compressed_shape);
    let reason = fits_refusal(
        HduSpec {
            header: &compressed,
            kind: HduKind::CompressedImage,
            shape: &compressed_shape,
            bitpix: Bitpix::F32,
            source_bytes: 2_880,
        }
        .description()
        .plan(
            FitsCubeInterpretation::Reject,
            FitsFloatScale::Auto,
            4 * 1024 * 1024 - 1,
        )
        .unwrap_err(),
    );
    assert_eq!(
        reason,
        FitsRefusal::OverBudget {
            allocation: FitsAllocation::DecodedOutput,
            budget: OverBudget {
                required: 4_194_304,
                limit: 4_194_303
            }
        }
    );
}

#[test]
fn header_rejection_precedes_pixel_read_and_truncated_data_is_an_error() {
    let dir = TempDir::new("fits_preflight");
    let path = dir.join("truncated.fits");
    let image = Image::new([2, 2], vec![1.0f32, 2.0, 3.0, 4.0]).unwrap();
    write_fits(&path, &image, None);
    let mut bytes = fs::read(&path).unwrap();
    bytes.truncate(BLOCK_SIZE);
    fs::write(&path, bytes).unwrap();

    let reason =
        fits_refusal(load_linear(&path, &LoadCtx::new(CancelToken::never(), 1)).unwrap_err());
    assert_eq!(
        reason,
        FitsRefusal::OverBudget {
            allocation: FitsAllocation::StoredDataUnit,
            budget: OverBudget {
                required: 2880,
                limit: 1
            }
        }
    );
    assert!(matches!(
        load_linear(&path, &LoadCtx::new(CancelToken::never(), 10_000)).unwrap_err(),
        ImageError::Fits { .. }
    ));
}

#[test]
fn zero_axis_file_returns_error_and_rgb_planes_load_without_repacking() {
    let dir = TempDir::new("fits_shape_and_rgb");
    let zero_path = dir.join("zero.fits");
    write_fits(
        &zero_path,
        &Image::new([0, 2], Vec::<f32>::new()).unwrap(),
        None,
    );
    let zero_axis = FitsRefusal::ZeroAxis { shape: vec![0, 2] };
    let reason = fits_refusal(load_linear(&zero_path, &load_ctx()).unwrap_err());
    assert_eq!(reason, zero_axis);
    let reason = fits_refusal(linear_info(&zero_path, &load_ctx()).unwrap_err());
    assert_eq!(reason, zero_axis, "the peek refuses what the load refuses");

    let rgb_path = dir.join("rgb.fits");
    let planar = vec![
        1.0f32, 2.0, 3.0, 4.0, 10.0, 20.0, 30.0, 40.0, 100.0, 200.0, 300.0, 400.0,
    ];
    write_fits(&rgb_path, &Image::new([2, 2, 3], planar).unwrap(), None);
    let loaded = load_linear(&rgb_path, &rgb_load_ctx()).unwrap();
    assert_eq!(
        loaded.dimensions(),
        ImageDimensions::new((2, 2), ChannelLayout::Rgb)
    );
    // The decode reads one channel's two rows at a time: 4 samples of 4 bytes as stored, and as
    // many swapped into words, beside the frame.
    let peek = linear_info(&rgb_path, &rgb_load_ctx()).unwrap();
    assert_eq!(
        peek.dimensions,
        loaded.dimensions(),
        "the header gives what the decode makes"
    );
    assert_eq!(peek.decoder_bytes, 32);

    let mosaic_path = dir.join("mosaic.fits");
    cfa::save(
        &mosaic_path,
        &make_cfa(
            Size2us::new(2, 2),
            vec![0.25; 4],
            CfaType::Bayer(CfaPattern::Rggb),
        ),
    )
    .unwrap();
    assert!(matches!(
        linear_info(&mosaic_path, &load_ctx()),
        Err(ImageError::ScientificInputRejected {
            reason: ScientificRefusal::MosaicFits,
            ..
        })
    ));
    assert_eq!(loaded.channel(0).pixels(), &[1.0, 2.0, 3.0, 4.0]);
    assert_eq!(loaded.channel(1).pixels(), &[10.0, 20.0, 30.0, 40.0]);
    assert_eq!(loaded.channel(2).pixels(), &[100.0, 200.0, 300.0, 400.0]);
}

#[test]
fn compressed_rgb_is_preflighted_and_decoded_by_final_plane() {
    let dir = TempDir::new("fits_compressed_rgb");
    let path = dir.join("rgb.fits");
    let planar = vec![
        1.0f32, 2.0, 3.0, 4.0, 10.0, 20.0, 30.0, 40.0, 100.0, 200.0, 300.0, 400.0,
    ];
    let image = Image::new([2, 2, 3], planar).unwrap();
    FitsWriter::new(fs::create(&path).unwrap())
        .write_compressed_image(
            &image,
            Compression::GZIP,
            &CompressionOptions::tiled([2, 2, 1]),
            None,
        )
        .unwrap();

    let loaded = load_linear(&path, &rgb_load_ctx()).unwrap();
    assert_eq!(
        loaded.dimensions(),
        ImageDimensions::new((2, 2), ChannelLayout::Rgb)
    );
    assert_eq!(loaded.channel(0).pixels(), &[1.0, 2.0, 3.0, 4.0]);
    assert_eq!(loaded.channel(1).pixels(), &[10.0, 20.0, 30.0, 40.0]);
    assert_eq!(loaded.channel(2).pixels(), &[100.0, 200.0, 300.0, 400.0]);
}

#[test]
fn hdu_selection_and_cube_interpretation_are_explicit_and_recorded() {
    let dir = TempDir::new("fits_hdu_selection");
    let path = dir.join("multi.fits");
    write_named_multi_image(&path);

    let reason = fits_refusal(load_linear(&path, &load_ctx()).unwrap_err());
    assert_eq!(reason, FitsRefusal::SeveralImageHdus { images: 2 });

    let ambiguous_name = LoadCtx {
        fits: FitsLoadConfig {
            hdu: FitsHduSelector::Name {
                extname: "sci".to_string(),
                extver: None,
            },
            ..Default::default()
        },
        ..load_ctx()
    };
    let reason = fits_refusal(load_linear(&path, &ambiguous_name).unwrap_err());
    assert_eq!(
        reason,
        FitsRefusal::SeveralMatchingHdus {
            extname: "sci".to_owned(),
            extver: None,
            matches: 2
        }
    );
    assert_eq!(
        reason.to_string(),
        "2 HDUs match EXTNAME=\"sci\"; specify EXTVER"
    );

    let first = LoadCtx {
        fits: FitsLoadConfig {
            hdu: FitsHduSelector::Idx(1),
            ..Default::default()
        },
        ..load_ctx()
    };
    let first = load_linear(&path, &first).unwrap();
    assert_eq!(first.channel(0).pixels(), &[1.0, 2.0]);
    let FitsTransferProvenance { hdu, checksum, .. } = fits_transfer(&first);
    assert_eq!(hdu.idx, 1);
    assert_eq!(hdu.extname.as_deref(), Some("SCI"));
    assert_eq!(hdu.extver, Some(1));
    assert_eq!(checksum.datasum, FitsChecksumState::Absent);
    assert_eq!(checksum.checksum, FitsChecksumState::Absent);

    let rejected_cube = LoadCtx {
        fits: FitsLoadConfig {
            hdu: FitsHduSelector::Name {
                extname: "SCI".to_string(),
                extver: Some(2),
            },
            ..Default::default()
        },
        ..load_ctx()
    };
    let reason = fits_refusal(load_linear(&path, &rejected_cube).unwrap_err());
    assert_eq!(reason, FitsRefusal::CubeNeedsRgb);

    let rgb = LoadCtx {
        fits: FitsLoadConfig {
            hdu: FitsHduSelector::Name {
                extname: "SCI".to_string(),
                extver: Some(2),
            },
            cube: FitsCubeInterpretation::Rgb,
            checksum: FitsChecksumPolicy::VerifyIfPresent,
            // Samples of 10 to 40 with no DATAMAX: said to be normalized, or `Auto` refuses them.
            float_scale: FitsFloatScale::Normalized,
            unstated_bayer_pattern: None,
            pedestal: Pedestal::Unknown,
        },
        ..load_ctx()
    };
    let rgb = load_linear(&path, &rgb).unwrap();
    assert_eq!(rgb.channel(0).pixels(), &[10.0, 20.0]);
    assert_eq!(rgb.channel(1).pixels(), &[30.0, 40.0]);
    assert_eq!(rgb.channel(2).pixels(), &[50.0, 60.0]);
}

#[test]
fn checksum_policies_accept_absence_ignore_corruption_or_require_exact_validity() {
    let dir = TempDir::new("fits_checksum_policy");
    let absent_path = dir.join("absent.fits");
    let image = Image::new([2, 1], vec![1.0f32, 2.0]).unwrap();
    write_fits(&absent_path, &image, None);

    let verified_absent = load_linear(&absent_path, &load_ctx()).unwrap();
    let FitsTransferProvenance { checksum, .. } = fits_transfer(&verified_absent);
    assert_eq!(checksum.datasum, FitsChecksumState::Absent);
    assert_eq!(checksum.checksum, FitsChecksumState::Absent);

    let require = LoadCtx {
        fits: FitsLoadConfig {
            checksum: FitsChecksumPolicy::RequireValid,
            ..Default::default()
        },
        ..load_ctx()
    };
    let reason = fits_refusal(load_linear(&absent_path, &require).unwrap_err());
    assert_eq!(
        reason,
        FitsRefusal::ChecksumRequired {
            hdu: 0,
            report: ChecksumReport {
                datasum: ChecksumStatus::Absent,
                checksum: ChecksumStatus::Absent
            }
        }
    );

    let valid_path = dir.join("valid.fits");
    FitsWriter::new(fs::create(&valid_path).unwrap())
        .with_checksums()
        .write_image(&image, None)
        .unwrap();
    let valid = load_linear(&valid_path, &require).unwrap();
    let FitsTransferProvenance { checksum, .. } = fits_transfer(&valid);
    assert_eq!(checksum.datasum, FitsChecksumState::Valid);
    assert_eq!(checksum.checksum, FitsChecksumState::Valid);

    let mut corrupt = fs::read(&valid_path).unwrap();
    corrupt[BLOCK_SIZE] ^= 0x80;
    fs::write(&valid_path, corrupt).unwrap();
    let reason = fits_refusal(load_linear(&valid_path, &load_ctx()).unwrap_err());
    assert_eq!(
        reason,
        FitsRefusal::InvalidChecksum {
            hdu: 0,
            report: ChecksumReport {
                datasum: ChecksumStatus::Invalid,
                checksum: ChecksumStatus::Invalid
            }
        }
    );

    let ignore = LoadCtx {
        fits: FitsLoadConfig {
            checksum: FitsChecksumPolicy::Ignore,
            ..Default::default()
        },
        ..load_ctx()
    };
    let ignored = load_linear(&valid_path, &ignore).unwrap();
    let FitsTransferProvenance { checksum, .. } = fits_transfer(&ignored);
    assert_eq!(checksum.datasum, FitsChecksumState::NotChecked);
    assert_eq!(checksum.checksum, FitsChecksumState::NotChecked);
}

#[test]
fn cancellation_prevents_fits_selection() {
    let dir = TempDir::new("fits_cancel");
    let path = dir.join("frame.fits");
    write_fits(&path, &Image::new([2, 1], vec![1.0f32, 2.0]).unwrap(), None);
    let cancel = CancelToken::new();
    cancel.cancel();
    let ctx = LoadCtx::new(cancel, u64::MAX);
    assert!(matches!(
        load_linear(&path, &ctx),
        Err(ImageError::Cancelled { .. })
    ));
}

/// `DATAMAX` is read once, for the scale and for the saturation level. A level of 0, a "not
/// computed" placeholder, is no level: the float samples are taken as normalized, as with no
/// `DATAMAX` at all, and nothing is flagged.
#[test]
fn a_zero_datamax_is_no_saturation_level() {
    let dir = TempDir::new("fits_datamax");
    let path = dir.join("zero.fits");
    let image = Image::new([2, 1], vec![0.25f32, 0.75]).unwrap();
    let mut header = Header::new();
    header.set("DATAMAX", 0.0).unwrap();
    write_fits(&path, &image, Some(&header));
    let loaded = load_linear(&path, &load_ctx()).unwrap();
    assert_eq!(loaded.channel(0).pixels(), &[0.25, 0.75]);
    assert_eq!(loaded.metadata.header.data_max, None);
    assert!(loaded.flags().is_none());
    assert!(!loaded.metadata.derived.saturation_flagged());
}

/// `Auto` picks the one HDU that holds pixels: a header-only image extension (`NAXIS = 0`) beside
/// it is no second candidate.
#[test]
fn a_header_only_image_extension_is_no_second_image() {
    let dir = TempDir::new("fits_header_only");
    let path = dir.join("one_image.fits");
    let mut writer = FitsWriter::new(fs::create(&path).unwrap());
    let image = Image::new([2, 1], vec![0.25f32, 0.75]).unwrap();
    writer.write_image(&image, None).unwrap();
    let mut empty = Header::new();
    empty.set("XTENSION", "IMAGE").unwrap();
    empty.set("BITPIX", -32).unwrap();
    empty.set("NAXIS", 0).unwrap();
    empty.set("PCOUNT", 0).unwrap();
    empty.set("GCOUNT", 1).unwrap();
    writer.write_raw_hdu(&empty, &[]).unwrap();
    writer.into_inner().sync_all().unwrap();

    let loaded = load_linear(&path, &load_ctx()).unwrap();
    assert_eq!(loaded.channel(0).pixels(), &[0.25, 0.75]);
    assert_eq!(fits_transfer(&loaded).hdu.idx, 0);
}
