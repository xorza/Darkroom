use std::fs::File;
use std::path::Path;

use common::CancelToken;
use fits_well::FitsWriter;
use fits_well::header::Header;
use fits_well::image::{Compression, CompressionOptions, Image};
use fits_well::io::{BLOCK_SIZE, HduKind};

use crate::io::image::fits::*;
use crate::io::image::fits::options::{
    FitsChecksumPolicy, FitsCubeInterpretation, FitsHduSelector, FitsLoadOptions,
};
use crate::io::image::fits::provenance::{FitsChecksumState, FitsTransferProvenance};
use crate::io::image::{LoadContext, TransferProvenance};
use crate::testing::ScratchDirectory;

fn load_context() -> LoadContext {
    LoadContext::new(CancelToken::never(), u64::MAX)
}

fn rgb_load_context() -> LoadContext {
    LoadContext {
        fits: FitsLoadOptions {
            cube: FitsCubeInterpretation::Rgb,
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

fn write_image(path: &Path, image: &Image) {
    let mut bytes = Vec::new();
    FitsWriter::new(&mut bytes).write_image(image).unwrap();
    std::fs::write(path, bytes).unwrap();
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
        .write_image_with_header(
            &Image::new([2, 1], vec![1.0f32, 2.0]).unwrap(),
            &first_header,
        )
        .unwrap();

    let mut second_header = Header::new();
    second_header.set("EXTNAME", "SCI").unwrap();
    second_header.set("EXTVER", 2).unwrap();
    writer
        .write_image_with_header(
            &Image::new(
                [2, 1, 3],
                vec![10.0f32, 20.0, 30.0, 40.0, 50.0, 60.0],
            )
            .unwrap(),
            &second_header,
        )
        .unwrap();
}

fn description(header: &Header, kind: HduKind, source_bytes: u64) -> FitsHduDescription<'_> {
    FitsHduDescription {
        header,
        kind,
        source_bytes,
    }
}

fn unsupported_reason(error: ImageError) -> String {
    let ImageError::FitsUnsupported { reason, .. } = error else {
        panic!("expected unsupported FITS error, got {error:?}");
    };
    reason
}

#[test]
fn shape_validation_rejects_zero_overflow_and_unsupported_cubes_without_panicking() {
    let path = Path::new("untrusted.fits");
    for shape in [&[0, 2][..], &[2, 0], &[2, 2, 0]] {
        let reason = unsupported_reason(
            dimensions_from_shape(path, shape, FitsCubeInterpretation::Reject).unwrap_err(),
        );
        assert!(reason.contains("must be nonzero"), "{reason}");
    }

    let pixel_overflow =
        unsupported_reason(
            dimensions_from_shape(path, &[usize::MAX, 2], FitsCubeInterpretation::Reject)
                .unwrap_err(),
        );
    assert!(pixel_overflow.contains("pixel count overflows"));

    let sample_overflow =
        unsupported_reason(
            dimensions_from_shape(
                path,
                &[usize::MAX / 2 + 1, 1, 3],
                FitsCubeInterpretation::Rgb,
            )
            .unwrap_err(),
        );
    assert!(sample_overflow.contains("sample count overflows"));

    let huge_cube = image_header(-32, &[1_000_000_000, 1_000_000_000, 4]);
    let reason = unsupported_reason(
        preflight_fits_image(
            path,
            description(&huge_cube, HduKind::Primary, 0),
            FitsCubeInterpretation::Reject,
            u64::MAX,
        )
        .unwrap_err(),
    );
    assert_eq!(reason, "Unsupported channel count (NAXIS3): 4");
}

#[test]
fn preflight_enforces_source_output_and_peak_limits_at_exact_boundaries() {
    let path = Path::new("budget.fits");
    let rgb = image_header(-64, &[100, 100, 3]);
    let plan = preflight_fits_image(
        path,
        description(&rgb, HduKind::Primary, 241_920),
        FitsCubeInterpretation::Rgb,
        u64::MAX,
    )
    .unwrap();
    assert_eq!(plan.source_bytes, 241_920);
    assert_eq!(plan.decoded_bytes, 120_000);
    assert_eq!(plan.peak_bytes, 320_000);
    assert_eq!(plan.rows_per_chunk, 100);
    preflight_fits_image(
        path,
        description(&rgb, HduKind::Primary, 241_920),
        FitsCubeInterpretation::Rgb,
        plan.peak_bytes,
    )
    .unwrap();
    let reason = unsupported_reason(
        preflight_fits_image(
            path,
            description(&rgb, HduKind::Primary, 241_920),
            FitsCubeInterpretation::Rgb,
            plan.peak_bytes - 1,
        )
        .unwrap_err(),
    );
    assert!(reason.starts_with("estimated peak memory requires 320000 bytes"));

    let compressed = compressed_header(-32, &[1024, 1024]);
    let reason = unsupported_reason(
        preflight_fits_image(
            path,
            description(&compressed, HduKind::CompressedImage, 2_880),
            FitsCubeInterpretation::Reject,
            4 * 1024 * 1024 - 1,
        )
        .unwrap_err(),
    );
    assert!(reason.starts_with("decoded output requires 4194304 bytes"));
}

#[test]
fn header_rejection_precedes_pixel_read_and_truncated_data_is_an_error() {
    let directory = ScratchDirectory::new("fits_preflight");
    let path = directory.join("truncated.fits");
    let image = Image::new([2, 2], vec![1.0f32, 2.0, 3.0, 4.0]).unwrap();
    write_image(&path, &image);
    let mut bytes = std::fs::read(&path).unwrap();
    bytes.truncate(BLOCK_SIZE);
    std::fs::write(&path, bytes).unwrap();

    let reason = unsupported_reason(
        read_selected_image(
            &path,
            &LoadContext::new(CancelToken::never(), 1),
        )
        .unwrap_err(),
    );
    assert!(reason.starts_with("stored data unit requires 2880 bytes"));
    assert!(matches!(
        read_selected_image(
            &path,
            &LoadContext::new(CancelToken::never(), 10_000),
        )
        .unwrap_err(),
        ImageError::Fits { .. }
    ));
}

#[test]
fn zero_axis_file_returns_error_and_rgb_planes_load_without_repacking() {
    let directory = ScratchDirectory::new("fits_shape_and_rgb");
    let zero_path = directory.join("zero.fits");
    write_image(&zero_path, &Image::new([0, 2], Vec::<f32>::new()).unwrap());
    let reason = unsupported_reason(load_linear_fits(&zero_path, &load_context()).unwrap_err());
    assert!(reason.contains("must be nonzero"));

    let rgb_path = directory.join("rgb.fits");
    let planar = vec![
        1.0f32, 2.0, 3.0, 4.0, 10.0, 20.0, 30.0, 40.0, 100.0, 200.0, 300.0, 400.0,
    ];
    write_image(&rgb_path, &Image::new([2, 2, 3], planar).unwrap());
    let loaded = load_linear_fits(&rgb_path, &rgb_load_context()).unwrap();
    assert_eq!(loaded.dimensions(), ImageDimensions::new((2, 2), 3));
    assert_eq!(loaded.channel(0).pixels(), &[1.0, 2.0, 3.0, 4.0]);
    assert_eq!(loaded.channel(1).pixels(), &[10.0, 20.0, 30.0, 40.0]);
    assert_eq!(loaded.channel(2).pixels(), &[100.0, 200.0, 300.0, 400.0]);
}

#[test]
fn compressed_rgb_is_preflighted_and_decoded_by_final_plane() {
    let directory = ScratchDirectory::new("fits_compressed_rgb");
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
        )
        .unwrap();

    let loaded = load_linear_fits(&path, &rgb_load_context()).unwrap();
    assert_eq!(loaded.dimensions(), ImageDimensions::new((2, 2), 3));
    assert_eq!(loaded.channel(0).pixels(), &[1.0, 2.0, 3.0, 4.0]);
    assert_eq!(loaded.channel(1).pixels(), &[10.0, 20.0, 30.0, 40.0]);
    assert_eq!(loaded.channel(2).pixels(), &[100.0, 200.0, 300.0, 400.0]);
}

#[test]
fn hdu_selection_and_cube_interpretation_are_explicit_and_recorded() {
    let directory = ScratchDirectory::new("fits_hdu_selection");
    let path = directory.join("multi.fits");
    write_named_multi_image(&path);

    let reason = unsupported_reason(load_linear_fits(&path, &load_context()).unwrap_err());
    assert_eq!(
        reason,
        "FITS file contains 2 image HDUs; select one explicitly by index or EXTNAME/EXTVER"
    );

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
    let reason = unsupported_reason(load_linear_fits(&path, &ambiguous_name).unwrap_err());
    assert_eq!(reason, "2 HDUs match EXTNAME=\"sci\"; specify EXTVER");

    let first = LoadContext {
        fits: FitsLoadOptions {
            hdu: FitsHduSelector::Index(1),
            ..Default::default()
        },
        ..load_context()
    };
    let first = load_linear_fits(&path, &first).unwrap();
    assert_eq!(first.channel(0).pixels(), &[1.0, 2.0]);
    let TransferProvenance::FitsPhysical(FitsTransferProvenance { hdu, checksum, .. }) =
        &first.metadata.provenance.as_ref().unwrap().transfer
    else {
        panic!("expected FITS provenance");
    };
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
    let reason = unsupported_reason(load_linear_fits(&path, &rejected_cube).unwrap_err());
    assert_eq!(
        reason,
        "three-plane FITS cube requires FitsCubeInterpretation::Rgb"
    );

    let rgb = LoadContext {
        fits: FitsLoadOptions {
            hdu: FitsHduSelector::Name {
                extname: "SCI".to_string(),
                extver: Some(2),
            },
            cube: FitsCubeInterpretation::Rgb,
            checksum: FitsChecksumPolicy::VerifyIfPresent,
        },
        ..load_context()
    };
    let rgb = load_linear_fits(&path, &rgb).unwrap();
    assert_eq!(rgb.channel(0).pixels(), &[10.0, 20.0]);
    assert_eq!(rgb.channel(1).pixels(), &[30.0, 40.0]);
    assert_eq!(rgb.channel(2).pixels(), &[50.0, 60.0]);
}

#[test]
fn checksum_policies_accept_absence_ignore_corruption_or_require_exact_validity() {
    let directory = ScratchDirectory::new("fits_checksum_policy");
    let absent_path = directory.join("absent.fits");
    let image = Image::new([2, 1], vec![1.0f32, 2.0]).unwrap();
    write_image(&absent_path, &image);

    let verified_absent = load_linear_fits(&absent_path, &load_context()).unwrap();
    let TransferProvenance::FitsPhysical(FitsTransferProvenance { checksum, .. }) =
        &verified_absent.metadata.provenance.as_ref().unwrap().transfer
    else {
        panic!("expected FITS provenance");
    };
    assert_eq!(checksum.datasum, FitsChecksumState::Absent);
    assert_eq!(checksum.checksum, FitsChecksumState::Absent);

    let require = LoadContext {
        fits: FitsLoadOptions {
            checksum: FitsChecksumPolicy::RequireValid,
            ..Default::default()
        },
        ..load_context()
    };
    let reason = unsupported_reason(load_linear_fits(&absent_path, &require).unwrap_err());
    assert!(reason.contains("requires valid DATASUM and CHECKSUM"));

    let valid_path = directory.join("valid.fits");
    FitsWriter::new(File::create(&valid_path).unwrap())
        .with_checksums()
        .write_image(&image)
        .unwrap();
    let valid = load_linear_fits(&valid_path, &require).unwrap();
    let TransferProvenance::FitsPhysical(FitsTransferProvenance { checksum, .. }) =
        &valid.metadata.provenance.as_ref().unwrap().transfer
    else {
        panic!("expected FITS provenance");
    };
    assert_eq!(checksum.datasum, FitsChecksumState::Valid);
    assert_eq!(checksum.checksum, FitsChecksumState::Valid);

    let mut corrupt = std::fs::read(&valid_path).unwrap();
    corrupt[BLOCK_SIZE] ^= 0x80;
    std::fs::write(&valid_path, corrupt).unwrap();
    let reason = unsupported_reason(load_linear_fits(&valid_path, &load_context()).unwrap_err());
    assert!(reason.contains("invalid FITS checksum"));

    let ignore = LoadContext {
        fits: FitsLoadOptions {
            checksum: FitsChecksumPolicy::Ignore,
            ..Default::default()
        },
        ..load_context()
    };
    let ignored = load_linear_fits(&valid_path, &ignore).unwrap();
    let TransferProvenance::FitsPhysical(FitsTransferProvenance { checksum, .. }) =
        &ignored.metadata.provenance.as_ref().unwrap().transfer
    else {
        panic!("expected FITS provenance");
    };
    assert_eq!(checksum.datasum, FitsChecksumState::NotChecked);
    assert_eq!(checksum.checksum, FitsChecksumState::NotChecked);
}

#[test]
fn cancellation_prevents_fits_selection_and_chunk_validation() {
    let directory = ScratchDirectory::new("fits_cancel");
    let path = directory.join("frame.fits");
    write_image(
        &path,
        &Image::new([2, 1], vec![1.0f32, 2.0]).unwrap(),
    );
    let cancel = CancelToken::new();
    cancel.cancel();
    let context = LoadContext::new(cancel.clone(), u64::MAX);
    assert!(matches!(
        load_linear_fits(&path, &context),
        Err(ImageError::Cancelled { .. })
    ));
    assert_eq!(
        validate_fits_pixels(vec![1.0, 2.0], &cancel).unwrap_err(),
        PixelValidationError::Cancelled
    );
}

#[test]
fn finite_validation_preserves_every_physical_value() {
    let pixels = vec![-5.0, 0.0, 0.5, 2.0, 255.0, 65_535.0];
    assert_eq!(
        validate_fits_pixels(pixels.clone(), &CancelToken::never()).unwrap(),
        pixels
    );
}

#[test]
fn non_finite_pixels_return_exact_summary_for_every_sample_type() {
    let pixels = vec![0.0, f32::NAN, 5.0, f32::INFINITY, f32::NEG_INFINITY];

    assert_eq!(
        validate_fits_pixels(pixels, &CancelToken::never()).unwrap_err(),
        PixelValidationError::Nulls(NullSummary {
            count: 3,
            first_index: 1,
        })
    );
}

#[test]
fn test_parse_sexagesimal_hms_to_ra_deg() {
    let expected = (5.0 + 35.0 / 60.0 + 17.3 / 3600.0) * 15.0;
    for sample in ["05 35 17.3", "05:35:17.3"] {
        let degrees = parse_sexagesimal(sample).unwrap() * 15.0;
        assert!(
            (degrees - expected).abs() < 1e-10,
            "{sample}: got {degrees}, expected {expected}"
        );
    }
    assert!((parse_sexagesimal("00 00 00.0").unwrap() * 15.0).abs() < 1e-10);
}

#[test]
fn test_parse_sexagesimal_dms_to_dec_deg() {
    let negative = parse_sexagesimal("-05 23 28.0").unwrap();
    assert!((negative - -(5.0 + 23.0 / 60.0 + 28.0 / 3600.0)).abs() < 1e-10);
    let positive = parse_sexagesimal("+45:30:15.5").unwrap();
    assert!((positive - (45.0 + 30.0 / 60.0 + 15.5 / 3600.0)).abs() < 1e-10);
    assert!((parse_sexagesimal("-00 30 00.0").unwrap() - -0.5).abs() < 1e-10);
}

#[test]
fn test_parse_sexagesimal_invalid() {
    assert!(parse_sexagesimal("05 35").is_none());
    assert!(parse_sexagesimal("").is_none());
    assert!(parse_sexagesimal("abc def ghi").is_none());
}
