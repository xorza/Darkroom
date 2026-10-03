#![expect(
    clippy::cast_possible_wrap,
    reason = "test axis lengths are far below i64::MAX"
)]

use crate::internals::prelude::*;

use std::fs::File;

use fits_well::FitsWriter;
use fits_well::header::Header;
use fits_well::image::{Bitpix, Compression, CompressionOptions, Image};
use fits_well::io::{BLOCK_SIZE, HduKind};

use crate::internals::fits::{fits_transfer, write_fits};
use crate::io::image::fits::decode::plan::internals::description;
use crate::io::image::fits::decode::*;
use crate::io::image::fits::options::{
    FitsChecksumPolicy, FitsFloatScale, FitsHduSelector, FitsLoadOptions, FitsNullPolicy,
};
use crate::io::image::fits::provenance::FitsTransferProvenance;
use common::TempDir;
use std::fs;

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
            plan::dimensions_from_shape(path, shape, FitsCubeInterpretation::Reject).unwrap_err(),
        );
        assert!(reason.contains("must be nonzero"), "{reason}");
    }

    // A side past the limit is refused before any count is formed from it; at the limit it is
    // accepted.
    let side = ImageDimensions::MAX_SIDE;
    for shape in [[side + 1, 2], [2, side + 1], [usize::MAX, 2]] {
        let reason = unsupported_reason(
            plan::dimensions_from_shape(path, &shape, FitsCubeInterpretation::Reject).unwrap_err(),
        );
        assert!(
            reason.contains("has a side past 1073741824 px"),
            "{shape:?}: {reason}"
        );
    }
    assert_eq!(
        plan::dimensions_from_shape(path, &[side, 1], FitsCubeInterpretation::Reject)
            .unwrap()
            .size(),
        (side, 1).into()
    );

    let huge_shape = [1_000_000_000, 1_000_000_000, 4];
    let huge_cube = image_header(-32, &huge_shape);
    let reason = unsupported_reason(
        plan::preflight_fits_image(
            path,
            description(&huge_cube, HduKind::Primary, &huge_shape, Bitpix::F32, 0),
            FitsCubeInterpretation::Reject,
            FitsFloatScale::Auto,
            u64::MAX,
        )
        .unwrap_err(),
    );
    assert_eq!(reason, "Unsupported channel count (NAXIS3): 4");
}

#[test]
fn preflight_enforces_source_output_and_peak_limits_at_exact_boundaries() {
    let path = Path::new("budget.fits");
    let rgb_shape = [100, 100, 3];
    let rgb = image_header(-64, &rgb_shape);
    let plan = plan::preflight_fits_image(
        path,
        description(&rgb, HduKind::Primary, &rgb_shape, Bitpix::F64, 241_920),
        FitsCubeInterpretation::Rgb,
        FitsFloatScale::Auto,
        u64::MAX,
    )
    .unwrap();
    assert_eq!(plan.source_bytes, 241_920);
    assert_eq!(plan.decoded_bytes, 120_000);
    assert_eq!(plan.peak_bytes, 320_000);
    assert_eq!(plan.rows_per_chunk, 100);
    plan::preflight_fits_image(
        path,
        description(&rgb, HduKind::Primary, &rgb_shape, Bitpix::F64, 241_920),
        FitsCubeInterpretation::Rgb,
        FitsFloatScale::Auto,
        plan.peak_bytes,
    )
    .unwrap();
    let reason = unsupported_reason(
        plan::preflight_fits_image(
            path,
            description(&rgb, HduKind::Primary, &rgb_shape, Bitpix::F64, 241_920),
            FitsCubeInterpretation::Rgb,
            FitsFloatScale::Auto,
            plan.peak_bytes - 1,
        )
        .unwrap_err(),
    );
    assert!(reason.starts_with("estimated peak memory requires 320000 bytes"));

    let compressed_shape = [1024, 1024];
    let compressed = compressed_header(-32, &compressed_shape);
    let reason = unsupported_reason(
        plan::preflight_fits_image(
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
    assert!(reason.starts_with("decoded output requires 4194304 bytes"));
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

    let reason = unsupported_reason(
        read_selected_image(&path, &LoadContext::new(CancelToken::never(), 1)).unwrap_err(),
    );
    assert!(reason.starts_with("stored data unit requires 2880 bytes"));
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
    let reason = unsupported_reason(load_linear_fits(&zero_path, &load_context()).unwrap_err());
    assert!(reason.contains("must be nonzero"));

    let rgb_path = directory.join("rgb.fits");
    let planar = vec![
        1.0f32, 2.0, 3.0, 4.0, 10.0, 20.0, 30.0, 40.0, 100.0, 200.0, 300.0, 400.0,
    ];
    write_fits(&rgb_path, &Image::new([2, 2, 3], planar).unwrap(), None);
    let loaded = load_linear_fits(&rgb_path, &rgb_load_context()).unwrap();
    assert_eq!(loaded.dimensions(), ImageDimensions::new((2, 2), 3));
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

    let loaded = load_linear_fits(&path, &rgb_load_context()).unwrap();
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
            float_scale: FitsFloatScale::Auto,
            nulls: FitsNullPolicy::Mask,
            unstated_bayer_pattern: None,
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
    let directory = TempDir::new("fits_checksum_policy");
    let absent_path = directory.join("absent.fits");
    let image = Image::new([2, 1], vec![1.0f32, 2.0]).unwrap();
    write_fits(&absent_path, &image, None);

    let verified_absent = load_linear_fits(&absent_path, &load_context()).unwrap();
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
    let reason = unsupported_reason(load_linear_fits(&absent_path, &require).unwrap_err());
    assert!(reason.contains("requires valid DATASUM and CHECKSUM"));

    let valid_path = directory.join("valid.fits");
    FitsWriter::new(File::create(&valid_path).unwrap())
        .with_checksums()
        .write_image(&image, None)
        .unwrap();
    let valid = load_linear_fits(&valid_path, &require).unwrap();
    let FitsTransferProvenance { checksum, .. } = fits_transfer(&valid);
    assert_eq!(checksum.datasum, FitsChecksumState::Valid);
    assert_eq!(checksum.checksum, FitsChecksumState::Valid);

    let mut corrupt = fs::read(&valid_path).unwrap();
    corrupt[BLOCK_SIZE] ^= 0x80;
    fs::write(&valid_path, corrupt).unwrap();
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
        load_linear_fits(&path, &context),
        Err(ImageError::Cancelled { .. })
    ));
}
