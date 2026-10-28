use common::{Buffer2, CancelToken, TempDir};
use libraw::internals::BayerDump;
use libraw::{LibrawCode, LibrawError, ProcessedSamples};

use crate::image::cfa::bayer_pattern::BayerPattern;
use crate::image::cfa::demosaic::xtrans::markesteijn::MarkesteijnPasses;
use crate::image::flags::quality_flags::{QualityFlags, SATURATION_FRACTION};
use crate::internals::cfa::XTRANS_PATTERN;
use crate::io::raw::unpacked_raw::{instrument, planes, saturation_level};
use crate::io::raw::*;
use common::USize2;
use glam::USizeVec2;

/// A dump at black `black`, with no other option.
fn black(black: u32) -> BayerDump {
    BayerDump {
        black,
        ..BayerDump::default()
    }
}

#[test]
fn load_invalid_path() {
    let path = Path::new("/nonexistent/path/to/file.raf");
    // A path that cannot be read is an `Io` error; a readable file libraw
    // refuses is a `Raw` one, pinned by `load_rejects_invalid_files`.
    let error = load_preview(path, &LoadCtx::default()).unwrap_err();
    assert!(
        matches!(&error, ImageError::Io { path: io_path, .. } if io_path == path),
        "a missing file should read as an Io error, got: {error}",
    );

    let cancel = CancelToken::new();
    cancel.cancel();
    let cancelled = LoadCtx::new(cancel);
    assert!(matches!(
        load_preview(path, &cancelled),
        Err(ImageError::Cancelled { path: error_path }) if error_path == path
    ));
    assert!(matches!(
        load_cfa(path, &cancelled),
        Err(ImageError::Cancelled { path: error_path }) if error_path == path
    ));
    assert!(matches!(
        cfa_peek(path, &cancelled),
        Err(ImageError::Cancelled { path: error_path }) if error_path == path
    ));
}

#[test]
fn load_rejects_invalid_files() {
    #[derive(Debug)]
    struct InvalidRawCase {
        name: &'static str,
        contents: &'static [u8],
    }

    let dir = TempDir::new("invalid_raw_files");
    let cases = [
        InvalidRawCase {
            name: "invalid_data",
            contents: b"not a valid raw file",
        },
        InvalidRawCase {
            name: "empty",
            contents: b"",
        },
    ];

    for case in cases {
        let path = dir.join(format!("{}.raf", case.name));
        fs::write(&path, case.contents).unwrap();
        assert!(
            load_preview(&path, &LoadCtx::default()).is_err(),
            "{case:?}"
        );

        // The rejection happens while libraw is being opened, so the handle exists and has to
        // free itself on the way out — the failure path most likely to leak the instance. The file
        // reads back, so what libraw refuses is its contents: a `Raw` error naming the open.
        let error = load_cfa(&path, &LoadCtx::default()).unwrap_err();
        assert!(
            matches!(
                &error,
                ImageError::Raw {
                    source: RawError::Libraw(LibrawError::Open(_)),
                    ..
                }
            ),
            "{case:?}: contents libraw refuses should read as a Raw open error, got: {error}",
        );
    }
}

/// A synthetic camera file through LibRaw's `open_bayer`, unpacked: the square `samples` under
/// one-pixel margins, RGGB, as `dump` describes it.
fn bayer_dump(samples: &Buffer2<u16>, dump: BayerDump) -> Result<UnpackedRaw, RawError> {
    let libraw = Libraw::open_bayer(samples, dump, &CancelToken::never())?;
    UnpackedRaw::unpack(libraw, 1)
}

/// The preview is the science frame demosaicked and clamped, bit for bit, and what lies in the
/// masked margins reaches neither: the same visible samples under black margins and under
/// saturated ones give the same image, its first rows and columns included, which the old preview
/// demosaicked across the margins. A bright square on a floor near black makes the demosaic
/// overshoot past 1 and the noise reach below 0 in the science frame; the preview holds both to
/// `[0, 1]`.
#[test]
fn the_preview_is_the_clamped_science_frame_and_ignores_the_margins() {
    const SIDE: usize = 26;
    let visible = |x: usize, y: usize| -> u16 {
        if (9..17).contains(&x) && (9..17).contains(&y) {
            60_000
        } else {
            1000 + ((x * 7 + y * 13) % 5) as u16 * 3
        }
    };
    let samples = |margin: u16| -> Buffer2<u16> {
        Buffer2::from_fn((SIDE, SIDE), |at| {
            if at.x == 0 || at.y == 0 || at.x == SIDE - 1 || at.y == SIDE - 1 {
                margin
            } else {
                visible(at.x - 1, at.y - 1)
            }
        })
    };
    let ctx = LoadCtx::default();
    let preview = |margin| {
        bayer_dump(&samples(margin), black(1004))
            .unwrap()
            .into_linear_image(&ctx)
    };
    let dark = preview(0).unwrap();
    let bright = preview(65_535).unwrap();
    let science = bayer_dump(&samples(0), black(1004))
        .unwrap()
        .into_cfa_image()
        .unwrap()
        .demosaic_measured(MarkesteijnPasses::One);
    assert_eq!(
        dark.desc(),
        PlanarDesc::new((SIDE - 2, SIDE - 2), ChannelLayout::Rgb)
    );
    let mut outside = 0;
    for channel in 0..3 {
        for ((&a, &b), &s) in dark
            .channel(channel)
            .pixels()
            .iter()
            .zip(bright.channel(channel).pixels())
            .zip(science.channel(channel).pixels())
        {
            assert_eq!(
                a.to_bits(),
                b.to_bits(),
                "the margins reached channel {channel}"
            );
            assert_eq!(a.to_bits(), s.clamp(0.0, 1.0).to_bits());
            outside += usize::from(!(0.0..=1.0).contains(&s));
        }
    }
    assert!(
        outside > 0,
        "the science frame leaves [0, 1], so the clamp is tested"
    );
    assert!(dark.metadata.derived.provenance().unwrap().clipped);
}

#[cfg(feature = "real-data")]
#[test]
fn load_valid_file() {
    use crate::internals::synthetic::metrics::pixel_stats;

    use crate::internals::init_tracing;
    use crate::internals::real_data::raw_frames;

    let path = raw_frames("Lights").swap_remove(0);

    init_tracing();

    let result = load_preview(&path, &LoadCtx::default());
    assert!(result.is_ok(), "Failed to load {path:?}: {result:?}");

    let image = result.unwrap();

    assert!(image.desc().width() > 0);
    assert!(image.desc().height() > 0);
    assert_eq!(image.desc().channel_count(), 3);

    // Every channel is inside the light-frame contract. The demosaic kernels overshoot on their
    // own, so this is what catches a decode path that stopped clamping their output.
    for c in 0..3 {
        for &pixel in image.channel(c) {
            assert!(
                (0.0..=1.0).contains(&pixel),
                "Pixel value {pixel} left [0, 1] in channel {c}"
            );
        }
    }

    let mean = (0..image.channel_count())
        .map(|channel| pixel_stats(image.channel(channel)).mean)
        .sum::<f64>()
        / image.channel_count() as f64;
    assert!(mean > 0.0, "Mean is zero, image may be all black");
    assert!(mean < 1.0, "Mean is >= 1.0, image may be overexposed");
}

/// LibRaw's own processing hands back the sensor's rows and columns: the visible area's width and
/// height, not turned by the EXIF orientation nor stretched by the pixel aspect — the settings
/// the fallback sets. LibRaw turns by the flip it saved at unpack unless `user_flip` overrides it,
/// and no sample is stored turned, so each is given a portrait one (flip 6, a quarter turn) first.
#[cfg(feature = "real-data")]
#[test]
fn the_fallback_keeps_the_sensors_rows_and_columns() {
    use crate::internals::real_data::raw_frames;

    for path in raw_frames("raw_samples") {
        let ctx = LoadCtx::default();
        let file = read(&path, &ctx).unwrap();
        let mut raw = open_raw(file, &path, &ctx).unwrap();
        let visible = raw.layout.active;
        assert_ne!(
            visible.width, visible.height,
            "a square frame cannot show a turn"
        );
        raw.libraw.set_saved_flip(6);
        let processed = raw.processed_by_libraw(None).unwrap();
        assert_eq!(processed.size(), visible, "{}", path.display());
    }
}

#[test]
fn camera_white_balance_is_canonicalized() {
    let bayer = Some(CfaType::Mosaic(Mosaic::Bayer(BayerPattern::Rggb)));
    assert_eq!(
        camera_white_balance(bayer, [4.0, 2.0, 3.0, 2.0], false),
        Some([2.0, 1.0, 1.5, 1.0])
    );
    assert_eq!(
        camera_white_balance(bayer, [2.0, 1.0, 1.5, 0.0], false),
        Some([2.0, 1.0, 1.5, 1.0])
    );
    assert_eq!(
        camera_white_balance(
            Some(CfaType::Mosaic(Mosaic::XTrans(XTRANS_PATTERN))),
            [2.0, 1.0, 1.5, 9.0],
            false
        ),
        Some([2.0, 1.0, 1.5, 1.0])
    );
    assert_eq!(
        camera_white_balance(Some(CfaType::Mono), [2.0, 1.0, 1.5, 1.0], false),
        None
    );
    // Samples that already carry the as-shot balance have unity left to apply, whatever the
    // multipliers say; a monochrome sensor has none either way.
    assert_eq!(
        camera_white_balance(bayer, [4.0, 2.0, 3.0, 2.0], true),
        Some([1.0; 4])
    );
    assert_eq!(
        camera_white_balance(Some(CfaType::Mono), [4.0, 2.0, 3.0, 2.0], true),
        None
    );
    // A sensor LibRaw processes itself (a linear DNG) still reports its multipliers.
    assert_eq!(
        camera_white_balance(None, [4.0, 2.0, 3.0, 2.0], false),
        Some([2.0, 1.0, 1.5, 1.0])
    );
}

#[test]
fn invalid_camera_white_balance_is_absent() {
    let invalid = [
        [0.0; 4],
        [2.0, -1.0, 1.5, 1.0],
        [2.0, f32::NAN, 1.5, 1.0],
        [2.0, f32::INFINITY, 1.5, 1.0],
    ];
    let cfa_type = Some(CfaType::Mosaic(Mosaic::Bayer(BayerPattern::Rggb)));

    for input in invalid {
        assert!(
            camera_white_balance(cfa_type, input, false).is_none(),
            "{input:?}"
        );
    }
}

/// A channel saturates `LINEAR_MAX_MARGIN` of its span below a stated linear limit that lies above its black
/// and within `maximum`, and 95% of the way from black to `maximum` otherwise. At black 1000,
/// maximum 16383:
/// - linear limit 15000 is one: 15000 − 0.005 × 14000 = 14930;
/// - 16383, at `maximum` itself: 16383 − 0.005 × 15383 = 16306.085;
/// - 0, none stated: 1000 + 0.95 × 15383 = 15613.85;
/// - 900, below black, and 1000, at it: read at the wrong scale, so `maximum`'s 15613.85;
/// - 65535, above `maximum`: the same.
#[test]
fn a_stated_linear_limit_sets_the_saturation_level_within_its_bounds() {
    let fallback = 1000.0 + f64::from(SATURATION_FRACTION) * 15383.0;
    for (linear_max, expected) in [
        (15_000, 14_930.0),
        (16_383, 16_383.0 - 0.005 * 15_383.0),
        (0, fallback),
        (900, fallback),
        (1000, fallback),
        (65_535, fallback),
    ] {
        assert_eq!(
            saturation_level(1000.0, linear_max, 16_383),
            expected,
            "linear_max {linear_max}"
        );
    }
}

/// The raw values settle two flags at decode. LibRaw's `open_bayer`
/// stands in for a camera file: it sets `zero_is_bad` from `procflags & 2`, `maximum` to
/// 65536 − 2⁰ = 65535 and black to its argument, here 1000. The saturation level is then
/// 1000 + 0.95 × (65535 − 1000) = 62308.25.
///
/// The 24 × 24 buffer reads 2000 everywhere, under one-pixel margins, except:
/// - a zero at raw (5, 7), visible (4, 6): `NO_DATA` when the camera says zeros are dead;
/// - a zero at raw (0, 0) in the masked margin, which is not part of the image;
/// - 62309 at raw (10, 10), visible (9, 9): saturated;
/// - 62308 at raw (12, 12), visible (11, 11): just below the level.
#[test]
fn the_raw_values_settle_no_data_and_saturation() {
    const SIDE: usize = 24;
    let dump = |procflags: u8| {
        let mut samples = Buffer2::new_filled((SIDE, SIDE), 2000u16);
        samples[7 * SIDE + 5] = 0;
        samples[0] = 0;
        samples[10 * SIDE + 10] = 62_309;
        samples[12 * SIDE + 12] = 62_308;
        bayer_dump(
            &samples,
            BayerDump {
                procflags,
                ..black(1000)
            },
        )
        .unwrap()
    };
    let flags_for = |procflags| {
        let raw = dump(procflags);
        raw.decode(raw.libraw.raw_image().unwrap()).flags.unwrap()
    };

    let flags = flags_for(2);
    assert_eq!(flags.size(), USize2::new(SIDE - 2, SIDE - 2));
    assert_eq!(flags.count(QualityFlags::NO_DATA), 1);
    assert_eq!(flags.at_pos(USizeVec2::new(4, 6)), QualityFlags::NO_DATA);
    assert_eq!(flags.count(QualityFlags::SATURATED), 1);
    assert_eq!(flags.at_pos(USizeVec2::new(9, 9)), QualityFlags::SATURATED);
    assert_eq!(
        flags.at_pos(USizeVec2::new(11, 11)),
        QualityFlags::default()
    );
    // A camera without the convention reads its zero as a value: not missing, not saturated.
    let flags = flags_for(0);
    assert_eq!(flags.count(QualityFlags::NO_DATA), 0);
    assert_eq!(flags.at_pos(USizeVec2::new(4, 6)), QualityFlags::default());

    // The frame keeps its dead photosite at the median of the others, as a FITS null is kept:
    // 2000 at every visible pixel but three, so the median is the floor visible (3, 3) holds.
    // Read as a value, the zero normalizes below black, to −1000 / span.
    let at = |image: &CfaImage, x: usize, y: usize| image.data()[y * (SIDE - 2) + x];
    let masked = dump(2).into_cfa_image().unwrap();
    assert_eq!(at(&masked, 4, 6).to_bits(), at(&masked, 3, 3).to_bits());
    let measured = dump(0).into_cfa_image().unwrap();
    assert!(at(&measured, 4, 6) < 0.0, "{}", at(&measured, 4, 6));
    assert_eq!(at(&measured, 3, 3).to_bits(), at(&masked, 3, 3).to_bits());
}

/// LibRaw measures a camera's black on its masked pixels and keeps each channel's mean truncated,
/// after `unpack` moved the least of them into `black`; the decode uses the means themselves.
/// The masked raw columns 0 and 1 hold 13 pixels of each of the four parities `p` (row parity
/// times 2 plus column parity), the one at row pair `r` worth `2048 + p + [r ≤ p]`: a sum of
/// 13·(2048 + p) + p + 1, so a mean of `2048 + p + (p + 1)/13`. LibRaw keeps 2048 + p, a loss
/// of (p + 1)/13 ADU in each channel.
#[test]
fn the_decode_takes_the_masked_means_libraw_truncates() {
    const SIDE: usize = 26;
    let parity = |row: usize, col: usize| (row % 2) * 2 + col % 2;
    let samples = Buffer2::from_fn((SIDE, SIDE), |at| {
        if at.x < 2 {
            let p = parity(at.y, at.x);
            (2048 + p + usize::from(at.y / 2 <= p)) as u16
        } else {
            3000
        }
    });
    let raw = bayer_dump(
        &samples,
        BayerDump {
            mask: Some([0, 0, SIDE as i32, 2]),
            ..BayerDump::default()
        },
    )
    .unwrap();
    let color = raw.libraw.color();
    assert_eq!(
        color.black, 2048,
        "unpack moved the least truncated mean into black"
    );
    for (row, col) in [(0, 0), (0, 1), (1, 0), (1, 1)] {
        let p = parity(row, col);
        let channel = filter_color(raw.visible_filters, row, col);
        let mean = 2048.0 + p as f64 + (p + 1) as f64 / 13.0;
        assert_eq!(color.black + color.cblack[channel], 2048 + p as u32);
        assert!(
            (raw.black_level.of_channel(channel) - mean).abs() < 1e-9,
            "parity {p}, channel {channel}: {} against {mean}",
            raw.black_level.of_channel(channel)
        );
    }
}

/// A sample above the bit depth the file declares is data LibRaw decodes past: `otherflags` 0x20
/// narrows the 16-bit samples to 14, a maximum of 2¹⁴ − 1 = 16383, so one visible sample of 20 000
/// makes the unpack a `CorruptData` error. The same sample is valid at 16 bits, and 16 000 is valid
/// at 14.
#[test]
fn a_value_past_the_bit_depth_is_corrupt_data() {
    const SIDE: usize = 24;
    let mut samples = Buffer2::new_filled((SIDE, SIDE), 2000u16);
    samples[10 * SIDE + 10] = 20_000;
    let narrow = BayerDump {
        otherflags: 0x20,
        ..BayerDump::default()
    };
    assert!(matches!(
        bayer_dump(&samples, narrow),
        Err(RawError::Libraw(LibrawError::CorruptData))
    ));
    assert!(bayer_dump(&samples, BayerDump::default()).is_ok());
    samples[10 * SIDE + 10] = 16_000;
    assert!(bayer_dump(&samples, narrow).is_ok());
}

/// A cancel the load's token sees stops LibRaw at its next stage, and reads as the load's own
/// cancel rather than as a failure of the file.
#[test]
fn a_cancelled_token_stops_libraw() {
    let samples = Buffer2::new_filled((24, 24), 2000u16);
    let cancel = CancelToken::new();
    let libraw = Libraw::open_bayer(&samples, BayerDump::default(), &cancel).unwrap();
    cancel.cancel();
    let path = Path::new("bayer-dump");
    let error = UnpackedRaw::unpack(libraw, 1).unwrap_err();
    assert!(
        matches!(
            error,
            RawError::Libraw(LibrawError::Unpack(LibrawCode::Cancelled))
        ),
        "{error:?}"
    );
    assert!(matches!(
        ImageError::raw(path, error),
        ImageError::Cancelled { .. }
    ));
}

/// The peek counts what LibRaw holds: the 24 × 24 × 2 = 1152-byte file it parses in place until
/// the sensor data is unpacked, and the raw buffer of as many samples it unpacks into, beside the
/// frame.
#[test]
fn the_peek_counts_the_file_and_the_raw_buffer() {
    let samples = Buffer2::new_filled((24, 24), 2000u16);
    let libraw = Libraw::open_bayer(&samples, BayerDump::default(), &CancelToken::never()).unwrap();
    let info = frame_info(&libraw).unwrap();
    assert_eq!(info.desc, PlanarDesc::new((22, 22), ChannelLayout::Mono));
    assert_eq!(
        info.decoder,
        DecoderBytes {
            source: 1152,
            beside: 1152
        }
    );
}

/// Once the sensor data is unpacked, the file goes and the raw buffer stays as it was unpacked:
/// the mosaic reads LibRaw's own memory alone.
#[test]
fn the_file_goes_once_the_sensor_data_is_unpacked() {
    let samples = Buffer2::from_fn((24, 24), |at| 1000 + (at.x * 24 + at.y) as u16);
    let mut libraw =
        Libraw::open_bayer(&samples, BayerDump::default(), &CancelToken::never()).unwrap();
    libraw.unpack(1).unwrap();
    let raw = libraw.raw_image().unwrap().to_vec();
    libraw.release_file();
    assert_eq!(libraw.file_len(), 0);
    assert_eq!(libraw.raw_image().unwrap(), raw);
}

/// LibRaw's own processing, the path of a sensor lumos does not demosaic, run on a uniform field:
/// 2000 over a black of 1000, a span of 65535 − 1000 = 64 535, comes out as three equal planes of
/// LibRaw's 16-bit 1000 · 65535 / 64 535 over 65535 — within one 16-bit step of 1000 / 64 535 —
/// and declares that span.
#[test]
fn libraws_processing_declares_its_own_span() {
    const SIDE: usize = 24;
    let samples = Buffer2::new_filled((SIDE, SIDE), 2000u16);
    let image = bayer_dump(&samples, black(1000))
        .unwrap()
        .processed_by_libraw(None)
        .unwrap();
    assert_eq!(image.desc(), PlanarDesc::new((22, 22), ChannelLayout::Rgb));
    assert_eq!(image.metadata.derived.domain().unwrap().scale, 64_535.0);
    let expected = 1000.0 / 64_535.0;
    for channel in 0..3 {
        for &value in image.channel(channel).pixels() {
            assert!(
                (f64::from(value) - expected).abs() <= 1.0 / 65_535.0,
                "channel {channel}: {value}"
            );
        }
    }
}

/// The camera is LibRaw's normalized make and model, each trimmed, joined by a space; either
/// alone when the other is empty, and `None` when both are.
#[test]
fn the_instrument_joins_the_normalized_make_and_model() {
    for (make, model, expected) in [
        ("Canon", "EOS 6D", Some("Canon EOS 6D")),
        ("Canon ", " EOS 6D", Some("Canon EOS 6D")),
        ("ZWO", "", Some("ZWO")),
        ("", "ASI2600MC", Some("ASI2600MC")),
        ("", "", None),
    ] {
        assert_eq!(
            instrument(make, model).as_deref(),
            expected,
            "{make:?} {model:?}"
        );
    }
}

/// LibRaw's processed samples, three colors interleaved a pixel, become one plane a color over
/// 65535: pixel 1's green, 65535, is 1 exactly, and pixel 0's blue, 0, is 0.
#[test]
fn processed_samples_split_into_one_plane_a_color() {
    let samples = [100, 200, 300, 400, 65_535, 0];
    let processed = ProcessedSamples {
        samples: &samples,
        size: USize2::new(2, 1),
        colors: 3,
    };
    let split: Vec<Vec<f32>> = planes(&processed).collect();
    assert_eq!(
        split,
        [
            vec![100.0 / 65535.0, 400.0 / 65535.0],
            vec![200.0 / 65535.0, 1.0],
            vec![300.0 / 65535.0, 0.0],
        ]
    );
}
