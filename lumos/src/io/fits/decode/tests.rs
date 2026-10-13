use crate::internals::prelude::*;

use crate::image::sample_domain::Pedestal;

use fits_well::FitsWriter;
use fits_well::header::Header;
use fits_well::image::{Bitpix, Compression, CompressionOptions, Image};
use fits_well::io::{BLOCK_SIZE, ChecksumReport, ChecksumStatus, HduKind};

use crate::image::cfa::cfa_pattern::CfaPattern;
use crate::internals::cfa::make_cfa;
use crate::internals::fits::{fits_refusal, fits_transfer, write_fits};
use crate::io::error::FitsAllocation;
use crate::io::fits::decode::plan::internals::description;
use crate::io::fits::decode::*;
use crate::io::fits::fits_checksum_provenance::FitsChecksumState;
use crate::io::fits::fits_load_options::{
    FitsChecksumPolicy, FitsFloatScale, FitsHduSelector, FitsLoadOptions, FitsNullPolicy,
};
use crate::io::fits::fits_transfer_provenance::FitsTransferProvenance;
use common::TempDir;
use std::fs;

fn load_context() -> LoadContext {
    LoadContext::new(CancelToken::never(), u64::MAX)
}

/// A cube read as RGB. Its float samples run to 400 with no `DATAMAX`, so they are said to be
/// normalized: these tests are about plane layout, not scale.
fn rgb_load_context() -> LoadContext {
    LoadContext {
        fits: FitsLoadOptions {
            cube: FitsCubeInterpretation::Rgb,
            float_scale: FitsFloatScale::Normalized,
            ..Default::default()
        },
        ..load_context()
    }
}

fn image_header(bitpix: i64, shape: &[usize]) -> Header {
    let mut header = Header::new();
    header.set("SIMPLE", true).unwrap();
    header.set("BITPIX", bitpix).unwrap();
    header.set("NAXIS", shape.len() as i64).unwrap();
    for (index, &axis) in shape.iter().enumerate() {
        header
            .set(&format!("NAXIS{}", index + 1), i64::try_from(axis).unwrap())
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
    for (index, &axis) in shape.iter().enumerate() {
        header
            .set(
                &format!("ZNAXIS{}", index + 1),
                i64::try_from(axis).unwrap(),
            )
            .unwrap();
    }
    header
}

fn write_named_multi_image(path: &Path) {
    let mut writer = FitsWriter::new(File::create(path).unwrap());
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
    let path = Path::new("untrusted.fits");
    for shape in [&[0, 2][..], &[2, 0], &[2, 2, 0]] {
        let reason = fits_refusal(
            plan::dimensions_from_shape(path, shape, FitsCubeInterpretation::Reject).unwrap_err(),
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
            plan::dimensions_from_shape(path, &shape, FitsCubeInterpretation::Reject).unwrap_err(),
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
        plan::dimensions_from_shape(path, &[side, 1], FitsCubeInterpretation::Reject)
            .unwrap()
            .size(),
        (side, 1).into()
    );

    let huge_shape = [1_000_000_000, 1_000_000_000, 4];
    let huge_cube = image_header(-32, &huge_shape);
    let reason = fits_refusal(
        plan::preflight(
            path,
            description(&huge_cube, HduKind::Primary, &huge_shape, Bitpix::F32, 0),
            FitsCubeInterpretation::Reject,
            FitsFloatScale::Auto,
            u64::MAX,
        )
        .unwrap_err(),
    );
    assert_eq!(reason, FitsRefusal::PlaneCount { planes: 4 });
    assert_eq!(reason.to_string(), "Unsupported channel count (NAXIS3): 4");
}

#[test]
fn preflight_enforces_source_output_and_peak_limits_at_exact_boundaries() {
    let path = Path::new("budget.fits");
    let rgb_shape = [100, 100, 3];
    let rgb = image_header(-64, &rgb_shape);
    let plan = plan::preflight(
        path,
        description(&rgb, HduKind::Primary, &rgb_shape, Bitpix::F64, 241_920),
        FitsCubeInterpretation::Rgb,
        FitsFloatScale::Auto,
        u64::MAX,
    )
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
    plan::preflight(
        path,
        description(&rgb, HduKind::Primary, &rgb_shape, Bitpix::F64, 241_920),
        FitsCubeInterpretation::Rgb,
        FitsFloatScale::Auto,
        plan.peak_bytes,
    )
    .unwrap();
    let reason = fits_refusal(
        plan::preflight(
            path,
            description(&rgb, HduKind::Primary, &rgb_shape, Bitpix::F64, 241_920),
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
            required: 290_000,
            limit: 289_999
        }
    );
    assert_eq!(
        reason.to_string(),
        "estimated peak memory requires 290000 bytes, exceeding the FITS load budget of 289999 \
         bytes"
    );
    // A flags extension is held beside the decoded 120 000 bytes as read, and as the decode's own
    // flag plane: 2 bytes for each of the 10 000 pixels, 140 000 in all.
    plan.admit_flags_extension(path, 140_000).unwrap();
    let reason = fits_refusal(plan.admit_flags_extension(path, 139_999).unwrap_err());
    assert_eq!(
        reason,
        FitsRefusal::OverBudget {
            allocation: FitsAllocation::DecodedOutputWithFlags,
            required: 140_000,
            limit: 139_999
        }
    );

    // An integer image with no BLANK holds no null, so it has no flag plane until a DATAMAX gives
    // the decode a level to flag against: 40 000 decoded bytes of f32 and two chunks of 100 rows
    // at 2 bytes a sample, 80 000, then 10 000 more. Without the DATAMAX a flags extension adds
    // its own 10 000 bytes to the decoded 40 000 and no decode plane: 50 000.
    let mono_shape = [100, 100];
    let mut mono = image_header(16, &mono_shape);
    let plan_of = |header: &Header| {
        plan::preflight(
            path,
            description(header, HduKind::Primary, &mono_shape, Bitpix::I16, 20_160),
            FitsCubeInterpretation::Reject,
            FitsFloatScale::Auto,
            u64::MAX,
        )
        .unwrap()
    };
    let without = plan_of(&mono);
    assert_eq!(without.peak_bytes, 80_000);
    without.admit_flags_extension(path, 50_000).unwrap();
    assert!(without.admit_flags_extension(path, 49_999).is_err());
    mono.set("DATAMAX", 30_000.0).unwrap();
    assert_eq!(plan_of(&mono).peak_bytes, 90_000);

    let compressed_shape = [1024, 1024];
    let compressed = compressed_header(-32, &compressed_shape);
    let reason = fits_refusal(
        plan::preflight(
            path,
            description(
                &compressed,
                HduKind::CompressedImage,
                &compressed_shape,
                Bitpix::F32,
                2_880,
            ),
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
            required: 4_194_304,
            limit: 4_194_303
        }
    );
}

#[test]
fn header_rejection_precedes_pixel_read_and_truncated_data_is_an_error() {
    let directory = TempDir::new("fits_preflight");
    let path = directory.join("truncated.fits");
    let image = Image::new([2, 2], vec![1.0f32, 2.0, 3.0, 4.0]).unwrap();
    write_fits(&path, &image, None);
    let mut bytes = fs::read(&path).unwrap();
    bytes.truncate(BLOCK_SIZE);
    fs::write(&path, bytes).unwrap();

    let reason = fits_refusal(
        read_selected_image(&path, &LoadContext::new(CancelToken::never(), 1)).unwrap_err(),
    );
    assert_eq!(
        reason,
        FitsRefusal::OverBudget {
            allocation: FitsAllocation::StoredDataUnit,
            required: 2880,
            limit: 1
        }
    );
    assert!(matches!(
        read_selected_image(&path, &LoadContext::new(CancelToken::never(), 10_000),).unwrap_err(),
        ImageError::Fits { .. }
    ));
}

#[test]
fn zero_axis_file_returns_error_and_rgb_planes_load_without_repacking() {
    let directory = TempDir::new("fits_shape_and_rgb");
    let zero_path = directory.join("zero.fits");
    write_fits(
        &zero_path,
        &Image::new([0, 2], Vec::<f32>::new()).unwrap(),
        None,
    );
    let zero_axis = FitsRefusal::ZeroAxis { shape: vec![0, 2] };
    let reason = fits_refusal(load_linear(&zero_path, &load_context()).unwrap_err());
    assert_eq!(reason, zero_axis);
    let reason = fits_refusal(linear_peek(&zero_path, &load_context()).unwrap_err());
    assert_eq!(reason, zero_axis, "the peek refuses what the load refuses");

    let rgb_path = directory.join("rgb.fits");
    let planar = vec![
        1.0f32, 2.0, 3.0, 4.0, 10.0, 20.0, 30.0, 40.0, 100.0, 200.0, 300.0, 400.0,
    ];
    write_fits(&rgb_path, &Image::new([2, 2, 3], planar).unwrap(), None);
    let loaded = load_linear(&rgb_path, &rgb_load_context()).unwrap();
    assert_eq!(loaded.dimensions(), ImageDimensions::new((2, 2), 3));
    // The decode reads one channel's two rows at a time: 4 samples of 4 bytes as stored, and as
    // many swapped into words, beside the frame.
    let peek = linear_peek(&rgb_path, &rgb_load_context()).unwrap();
    assert_eq!(
        peek.dimensions,
        loaded.dimensions(),
        "the header gives what the decode makes"
    );
    assert_eq!(peek.decoder_bytes, 32);

    let mosaic_path = directory.join("mosaic.fits");
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
        linear_peek(&mosaic_path, &load_context()),
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
    let directory = TempDir::new("fits_compressed_rgb");
    let path = directory.join("rgb.fits");
    let planar = vec![
        1.0f32, 2.0, 3.0, 4.0, 10.0, 20.0, 30.0, 40.0, 100.0, 200.0, 300.0, 400.0,
    ];
    let image = Image::new([2, 2, 3], planar).unwrap();
    FitsWriter::new(File::create(&path).unwrap())
        .write_compressed_image(
            &image,
            Compression::GZIP,
            &CompressionOptions::tiled([2, 2, 1]),
            None,
        )
        .unwrap();

    let loaded = load_linear(&path, &rgb_load_context()).unwrap();
    assert_eq!(loaded.dimensions(), ImageDimensions::new((2, 2), 3));
    assert_eq!(loaded.channel(0).pixels(), &[1.0, 2.0, 3.0, 4.0]);
    assert_eq!(loaded.channel(1).pixels(), &[10.0, 20.0, 30.0, 40.0]);
    assert_eq!(loaded.channel(2).pixels(), &[100.0, 200.0, 300.0, 400.0]);
}

#[test]
fn hdu_selection_and_cube_interpretation_are_explicit_and_recorded() {
    let directory = TempDir::new("fits_hdu_selection");
    let path = directory.join("multi.fits");
    write_named_multi_image(&path);

    let reason = fits_refusal(load_linear(&path, &load_context()).unwrap_err());
    assert_eq!(reason, FitsRefusal::SeveralImageHdus { images: 2 });

    let ambiguous_name = LoadContext {
        fits: FitsLoadOptions {
            hdu: FitsHduSelector::Name {
                extname: "sci".to_string(),
                extver: None,
            },
            ..Default::default()
        },
        ..load_context()
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

    let first = LoadContext {
        fits: FitsLoadOptions {
            hdu: FitsHduSelector::Index(1),
            ..Default::default()
        },
        ..load_context()
    };
    let first = load_linear(&path, &first).unwrap();
    assert_eq!(first.channel(0).pixels(), &[1.0, 2.0]);
    let FitsTransferProvenance { hdu, checksum, .. } = fits_transfer(&first);
    assert_eq!(hdu.index, 1);
    assert_eq!(hdu.extname.as_deref(), Some("SCI"));
    assert_eq!(hdu.extver, Some(1));
    assert_eq!(checksum.datasum, FitsChecksumState::Absent);
    assert_eq!(checksum.checksum, FitsChecksumState::Absent);

    let rejected_cube = LoadContext {
        fits: FitsLoadOptions {
            hdu: FitsHduSelector::Name {
                extname: "SCI".to_string(),
                extver: Some(2),
            },
            ..Default::default()
        },
        ..load_context()
    };
    let reason = fits_refusal(load_linear(&path, &rejected_cube).unwrap_err());
    assert_eq!(reason, FitsRefusal::CubeNeedsRgb);

    let rgb = LoadContext {
        fits: FitsLoadOptions {
            hdu: FitsHduSelector::Name {
                extname: "SCI".to_string(),
                extver: Some(2),
            },
            cube: FitsCubeInterpretation::Rgb,
            checksum: FitsChecksumPolicy::VerifyIfPresent,
            // Samples of 10 to 40 with no DATAMAX: said to be normalized, or `Auto` refuses them.
            float_scale: FitsFloatScale::Normalized,
            nulls: FitsNullPolicy::Mask,
            unstated_bayer_pattern: None,
            pedestal: Pedestal::Unknown,
        },
        ..load_context()
    };
    let rgb = load_linear(&path, &rgb).unwrap();
    assert_eq!(rgb.channel(0).pixels(), &[10.0, 20.0]);
    assert_eq!(rgb.channel(1).pixels(), &[30.0, 40.0]);
    assert_eq!(rgb.channel(2).pixels(), &[50.0, 60.0]);
}

#[test]
fn checksum_policies_accept_absence_ignore_corruption_or_require_exact_validity() {
    let directory = TempDir::new("fits_checksum_policy");
    let absent_path = directory.join("absent.fits");
    let image = Image::new([2, 1], vec![1.0f32, 2.0]).unwrap();
    write_fits(&absent_path, &image, None);

    let verified_absent = load_linear(&absent_path, &load_context()).unwrap();
    let FitsTransferProvenance { checksum, .. } = fits_transfer(&verified_absent);
    assert_eq!(checksum.datasum, FitsChecksumState::Absent);
    assert_eq!(checksum.checksum, FitsChecksumState::Absent);

    let require = LoadContext {
        fits: FitsLoadOptions {
            checksum: FitsChecksumPolicy::RequireValid,
            ..Default::default()
        },
        ..load_context()
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

    let valid_path = directory.join("valid.fits");
    FitsWriter::new(File::create(&valid_path).unwrap())
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
    let reason = fits_refusal(load_linear(&valid_path, &load_context()).unwrap_err());
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

    let ignore = LoadContext {
        fits: FitsLoadOptions {
            checksum: FitsChecksumPolicy::Ignore,
            ..Default::default()
        },
        ..load_context()
    };
    let ignored = load_linear(&valid_path, &ignore).unwrap();
    let FitsTransferProvenance { checksum, .. } = fits_transfer(&ignored);
    assert_eq!(checksum.datasum, FitsChecksumState::NotChecked);
    assert_eq!(checksum.checksum, FitsChecksumState::NotChecked);
}

#[test]
fn cancellation_prevents_fits_selection() {
    let directory = TempDir::new("fits_cancel");
    let path = directory.join("frame.fits");
    write_fits(&path, &Image::new([2, 1], vec![1.0f32, 2.0]).unwrap(), None);
    let cancel = CancelToken::new();
    cancel.cancel();
    let context = LoadContext::new(cancel, u64::MAX);
    assert!(matches!(
        load_linear(&path, &context),
        Err(ImageError::Cancelled { .. })
    ));
}
