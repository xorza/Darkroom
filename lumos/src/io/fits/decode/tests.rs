use std::ops::Range;

use arrayvec::ArrayVec;
use common::{CancelToken, TempDir, fs};
use fits_well::compress::{Compression, CompressionConfig, Quantization};
use fits_well::header::Header;
use fits_well::image::{Bitpix, Image, Scaling};
use fits_well::io::{BLOCK_SIZE, ChecksumReport, ChecksumStatus};
use fits_well::{FitsReader, FitsWriter};

use crate::image::cfa::cfa_pattern::CfaPattern;
use crate::image::cfa::mosaic::Mosaic;
use crate::image::channel_layout::ChannelLayout;
use crate::image::fits_checksum_provenance::FitsChecksumState;
use crate::image::fits_transfer_provenance::FitsTransferProvenance;
use crate::image::image_dimensions::ImageDimensions;
use crate::image::sample_domain::Pedestal;
use crate::internals::cfa::make_cfa;
use crate::internals::fits::{fits_refusal, fits_transfer, write_fits};
use crate::io::fits::cfa;
use crate::io::fits::decode;
use crate::io::fits::decode::plan::internals::HduSpec;
use crate::io::fits::decode::*;
use crate::io::fits::fits_load_config::{
    FitsChecksumPolicy, FitsCubeInterpretation, FitsFloatScale, FitsHduSelector, FitsLoadConfig,
};
use crate::io::frame_info::DecoderBytes;
use common::Size2us;

/// [`decode::load_linear`] of a run of its own, which shares its gain with no other frame.
fn load_linear(path: &Path, ctx: &LoadCtx) -> Result<LinearImage, ImageError> {
    decode::load_linear(path, ctx, &SharedGains::default())
}

fn load_ctx() -> LoadCtx {
    LoadCtx::new(CancelToken::never())
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
            .set(format!("NAXIS{}", idx + 1), i64::try_from(axis).unwrap())
            .unwrap();
    }
    header
}

fn write_named_multi_image(path: &Path) {
    let mut writer = FitsWriter::new(fs::create(path).unwrap());
    writer.write_empty_primary(None).unwrap();

    let mut first_header = Header::new();
    first_header.set("EXTNAME", "SCI").unwrap();
    first_header.set("EXTVER", 1).unwrap();
    writer
        .write_image(
            Image::new([2, 1], vec![1.0f32, 2.0], Scaling::IDENTITY)
                .unwrap()
                .view(),
            Some(&first_header),
        )
        .unwrap();

    let mut second_header = Header::new();
    second_header.set("EXTNAME", "SCI").unwrap();
    second_header.set("EXTVER", 2).unwrap();
    writer
        .write_image(
            Image::new(
                [2, 1, 3],
                vec![10.0f32, 20.0, 30.0, 40.0, 50.0, 60.0],
                Scaling::IDENTITY,
            )
            .unwrap()
            .view(),
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
            shape: &huge_shape,
            bitpix: Bitpix::F32,
            source_bytes: 0,
        }
        .description()
        .plan(
            FitsCubeInterpretation::Reject,
            FitsFloatScale::Auto,
            None,
            |_| Ok(0),
        )
        .unwrap_err(),
    );
    assert_eq!(reason, FitsRefusal::PlaneCount { planes: 4 });
    assert_eq!(reason.to_string(), "unsupported channel count (NAXIS3): 4");
}

#[test]
fn preflight_sizes_the_source_the_output_and_the_peak() {
    let rgb_shape = [100, 100, 3];
    let rgb = image_header(-64, &rgb_shape);
    let plan = HduSpec {
        header: &rgb,
        shape: &rgb_shape,
        bitpix: Bitpix::F64,
        source_bytes: 241_920,
    }
    .description()
    .plan(
        FitsCubeInterpretation::Rgb,
        FitsFloatScale::Auto,
        None,
        |regions| {
            // A row of the three planes is 1 200 B of f32, so the 4 MiB chunk holds every row:
            // one chunk of all 100 rows, across all three planes.
            assert_eq!(regions, [ArrayVec::from([0..100, 0..100, 0..3])]);
            Ok(1_700)
        },
    )
    .unwrap();
    assert_eq!(plan.source_bytes, 241_920);
    assert_eq!(plan.decoded_bytes, 120_000);
    assert_eq!(plan.rows_per_chunk, 100);
    // The 1 700 B the reader holds, the 120 000 decoded bytes, the chunk's words — 30 000 samples
    // of 8 bytes, 240 000 — and the flag plane a float image may need for its NaNs, a byte for
    // each of the 10 000 pixels.
    assert_eq!(plan.peak_bytes, 1_700 + 120_000 + 240_000 + 10_000);
    // Beside the frame: the words and the reader, 241 700 B, and the largest extension read
    // after the image.
    for (extensions, beside) in [
        (&[][..], 241_700),
        (&[10_000][..], 251_700),
        (&[300_000, 400_000][..], 641_700),
        (&[400_000, 300_000][..], 641_700),
    ] {
        assert_eq!(
            plan.beside_frame_bytes(extensions),
            beside,
            "{extensions:?}"
        );
    }

    // An integer image with no BLANK holds no null, so it has no flag plane until a DATAMAX gives
    // the decode a level to flag against: 40 000 decoded bytes of f32 and a chunk of the 100 rows
    // at 2 bytes a sample, 20 000, then 10 000 more.
    let mono_shape = [100, 100];
    let mut mono = image_header(16, &mono_shape);
    let plan_of = |header: &Header| {
        HduSpec {
            header,
            shape: &mono_shape,
            bitpix: Bitpix::I16,
            source_bytes: 20_160,
        }
        .description()
        .plan(
            FitsCubeInterpretation::Reject,
            FitsFloatScale::Auto,
            None,
            |_| Ok(0),
        )
        .unwrap()
    };
    let without = plan_of(&mono);
    assert_eq!(without.peak_bytes, 60_000);
    mono.set("DATAMAX", 30_000.0).unwrap();
    assert_eq!(plan_of(&mono).peak_bytes, 70_000);
}

/// A compressed image's chunks are whole rows of its tiles: chunks of 256 rows of 4096 samples of
/// `f32` fill `CHUNK_BYTES`, so tiles a row high read 256 rows a chunk, tiles 100 rows high two of
/// their rows, 200, and one tile of every row the whole 300, which decodes once. Beside the
/// 4 915 200 decoded bytes, the peak holds a chunk's words, 2 bytes a sample, and what fits-well
/// says its reader holds viewing those chunks in turn and finishing the checksum; an integer image
/// with no `BLANK` has no flag plane.
#[test]
fn a_compressed_image_reads_whole_tile_rows_and_plans_what_its_reader_holds() {
    let dir = TempDir::new("fits_compressed_plan");
    let (width, height) = (4096, 300);
    let samples: Vec<i16> = (0..width * height).map(|i| (i % 251) as i16).collect();
    let image = Image::new([width, height], samples, Scaling::IDENTITY).unwrap();
    for (tile_rows, rows_per_chunk) in [(1, 256), (100, 200), (300, 300)] {
        let path = dir.join(format!("tiles_{tile_rows}.fits"));
        let mut writer = FitsWriter::new(fs::create(&path).unwrap());
        writer.write_empty_primary(None).unwrap();
        #[expect(
            clippy::disallowed_methods,
            reason = "the test's own write, outside every loop"
        )]
        writer
            .write_compressed_image(
                image.view(),
                &CompressionConfig::new(Compression::Rice).with_tiles([width, tile_rows]),
                None,
            )
            .unwrap();
        writer.into_inner().sync_all().unwrap();

        let ctx = load_ctx();
        let mut reader = selected_fits::open(&path, &ctx).unwrap();
        let plan = SelectedFits::select(&mut reader, &ctx).unwrap().plan;
        assert_eq!(plan.rows_per_chunk, rows_per_chunk, "{tile_rows}");

        let mut fits = FitsReader::open(fs::open(&path).unwrap()).unwrap();
        let regions: Vec<[Range<usize>; 2]> = (0..height)
            .step_by(rows_per_chunk)
            .map(|start| [0..width, start..(start + rows_per_chunk).min(height)])
            .collect();
        let image = fits.image(1).unwrap();
        let held = image
            .view_bytes(regions.iter().map(|ranges| ranges.as_slice().into()))
            .unwrap();
        let words = (width * rows_per_chunk * 2) as u64;
        assert_eq!(
            plan.peak_bytes,
            4_915_200 + words + held.kept + held.handle + image.finish_bytes(),
            "{tile_rows}"
        );
    }
}

#[test]
fn truncated_data_is_an_error() {
    let dir = TempDir::new("fits_preflight");
    let path = dir.join("truncated.fits");
    let image = Image::new([2, 2], vec![1.0f32, 2.0, 3.0, 4.0], Scaling::IDENTITY).unwrap();
    write_fits(&path, image.view(), None);
    let mut bytes = fs::read(&path).unwrap();
    bytes.truncate(BLOCK_SIZE);
    fs::write(&path, bytes).unwrap();

    assert!(matches!(
        load_linear(&path, &load_ctx()).unwrap_err(),
        ImageError::Fits { .. }
    ));
}

#[test]
fn zero_axis_file_returns_error_and_rgb_planes_load_without_repacking() {
    let dir = TempDir::new("fits_shape_and_rgb");
    let zero_path = dir.join("zero.fits");
    write_fits(
        &zero_path,
        Image::new([0, 2], Vec::<f32>::new(), Scaling::IDENTITY)
            .unwrap()
            .view(),
        None,
    );
    let zero_axis = FitsRefusal::ZeroAxis { shape: vec![0, 2] };
    let reason = fits_refusal(load_linear(&zero_path, &load_ctx()).unwrap_err());
    assert_eq!(reason, zero_axis);
    let reason = fits_refusal(linear_peek(&zero_path, &load_ctx()).unwrap_err());
    assert_eq!(reason, zero_axis, "the peek refuses what the load refuses");

    let rgb_path = dir.join("rgb.fits");
    let planar = vec![
        1.0f32, 2.0, 3.0, 4.0, 10.0, 20.0, 30.0, 40.0, 100.0, 200.0, 300.0, 400.0,
    ];
    write_fits(
        &rgb_path,
        Image::new([2, 2, 3], planar, Scaling::IDENTITY)
            .unwrap()
            .view(),
        None,
    );
    let loaded = load_linear(&rgb_path, &rgb_load_ctx()).unwrap();
    assert_eq!(
        loaded.dimensions(),
        ImageDimensions::new((2, 2), ChannelLayout::Rgb)
    );
    // The decode reads the two rows of the three planes at once: 12 samples of 4 bytes, swapped
    // into 48 B of words, beside the frame, the reader's section shape, 8 B for each of the
    // three axes, and the staging the checksum finish reads a stream's unit through, its
    // one padded block.
    let peek = linear_peek(&rgb_path, &rgb_load_ctx()).unwrap().info;
    assert_eq!(
        peek.dimensions,
        loaded.dimensions(),
        "the header gives what the decode makes"
    );
    assert_eq!(
        peek.decoder,
        DecoderBytes {
            source: 0,
            beside: 48 + 24 + BLOCK_SIZE
        }
    );

    let mosaic_path = dir.join("mosaic.fits");
    cfa::save(
        &mosaic_path,
        &make_cfa(
            Size2us::new(2, 2),
            vec![0.25; 4],
            CfaType::Mosaic(Mosaic::Bayer(CfaPattern::Rggb)),
        ),
    )
    .unwrap();
    assert!(matches!(
        linear_peek(&mosaic_path, &load_ctx()),
        Err(ImageError::ScientificInputRejected {
            reason: ScientificRefusal::MosaicFits,
            ..
        })
    ));
    assert_eq!(loaded.channel(0).pixels(), &[1.0, 2.0, 3.0, 4.0]);
    assert_eq!(loaded.channel(1).pixels(), &[10.0, 20.0, 30.0, 40.0]);
    assert_eq!(loaded.channel(2).pixels(), &[100.0, 200.0, 300.0, 400.0]);
}

/// A compressed cube loads plane by plane from tiles of one plane, and from tiles that span all
/// three, which a chunk reads across every plane so each tile decodes once. The peek keeps the
/// file and the tile index fits-well's reader read for it, which the load reads on from: the
/// path it is given is only what an error reports, so one that names no file still loads.
#[test]
fn compressed_rgb_loads_from_tiles_of_a_plane_or_of_every_plane() {
    let dir = TempDir::new("fits_compressed_rgb");
    let planar = vec![
        1.0f32, 2.0, 3.0, 4.0, 10.0, 20.0, 30.0, 40.0, 100.0, 200.0, 300.0, 400.0,
    ];
    let image = Image::new([2, 2, 3], planar, Scaling::IDENTITY).unwrap();
    for tiles in [[2, 2, 1], [2, 2, 3], [2, 1, 3]] {
        let path = dir.join(format!("rgb_{}_{}_{}.fits", tiles[0], tiles[1], tiles[2]));
        #[expect(
            clippy::disallowed_methods,
            reason = "the test's own write, outside every loop"
        )]
        FitsWriter::new(fs::create(&path).unwrap())
            .write_compressed_image(
                image.view(),
                &CompressionConfig::new(Compression::GZIP)
                    .with_tiles(tiles)
                    .with_quantization(Quantization::NONE),
                None,
            )
            .unwrap();

        let Peek { info, read: file } = linear_peek(&path, &rgb_load_ctx()).unwrap();
        let index = FitsReader::open(fs::open(&path).unwrap())
            .unwrap()
            .image(1)
            .unwrap()
            .view_bytes(std::iter::empty())
            .unwrap()
            .kept;
        assert!(
            index > 0,
            "{tiles:?}: a compressed image keeps its tile index"
        );
        assert_eq!(file.held_bytes() as u64, index, "{tiles:?}");
        let from_peek = load_linear_file(
            file,
            &dir.join("absent.fits"),
            &rgb_load_ctx(),
            &SharedGains::default(),
        )
        .unwrap();
        for loaded in [from_peek, load_linear(&path, &rgb_load_ctx()).unwrap()] {
            assert_eq!(loaded.dimensions(), info.dimensions, "{tiles:?}");
            assert_eq!(
                loaded.dimensions(),
                ImageDimensions::new((2, 2), ChannelLayout::Rgb)
            );
            assert_eq!(loaded.channel(0).pixels(), &[1.0, 2.0, 3.0, 4.0]);
            assert_eq!(loaded.channel(1).pixels(), &[10.0, 20.0, 30.0, 40.0]);
            assert_eq!(loaded.channel(2).pixels(), &[100.0, 200.0, 300.0, 400.0]);
        }
    }
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
    let image = Image::new([2, 1], vec![1.0f32, 2.0], Scaling::IDENTITY).unwrap();
    write_fits(&absent_path, image.view(), None);

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
        .write_image(image.view(), None)
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
    write_fits(
        &path,
        Image::new([2, 1], vec![1.0f32, 2.0], Scaling::IDENTITY)
            .unwrap()
            .view(),
        None,
    );
    let cancel = CancelToken::new();
    cancel.cancel();
    let ctx = LoadCtx::new(cancel);
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
    let image = Image::new([2, 1], vec![0.25f32, 0.75], Scaling::IDENTITY).unwrap();
    let mut header = Header::new();
    header.set("DATAMAX", 0.0).unwrap();
    write_fits(&path, image.view(), Some(&header));
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
    let image = Image::new([2, 1], vec![0.25f32, 0.75], Scaling::IDENTITY).unwrap();
    writer.write_image(image.view(), None).unwrap();
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
