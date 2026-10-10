use common::TempDir;

use crate::io::raw::demosaic::xtrans::markesteijn::MarkesteijnPasses;
use common::CancelToken;

use crate::internals::cfa::XTRANS_PATTERN;
use crate::io::image::pixel_flags::{QualityFlags, SATURATION_FRACTION};
use crate::io::raw::demosaic::bayer::CfaPattern;
use crate::io::raw::error::LibrawCode;
use crate::io::raw::libraw::internals::BayerDump;
use crate::io::raw::unpacked_raw::{instrument, saturation_level};
use crate::io::raw::*;
use crate::math::size2us::Size2us;
use crate::math::vec2us::Vec2us;

/// A dump at black `black`, with no other option.
fn black(black: u32) -> BayerDump {
    BayerDump {
        black,
        ..BayerDump::default()
    }
}

#[test]
fn load_raw_invalid_path() {
    let path = Path::new("/nonexistent/path/to/file.raf");
    // A path that cannot be read is an `Io` error; a readable file libraw
    // refuses is a `Raw` one, pinned by `load_raw_rejects_invalid_files`.
    let error = load_raw(path, &LoadContext::default()).unwrap_err();
    assert!(
        matches!(&error, ImageError::Io { path: io_path, .. } if io_path == path),
        "a missing file should read as an Io error, got: {error}",
    );

    let cancel = CancelToken::new();
    cancel.cancel();
    let cancelled = LoadContext::new(cancel, u64::MAX);
    assert!(matches!(
        load_raw(path, &cancelled),
        Err(ImageError::Cancelled { path: error_path }) if error_path == path
    ));
    assert!(matches!(
        load_raw_cfa(path, &cancelled),
        Err(ImageError::Cancelled { path: error_path }) if error_path == path
    ));
    assert!(matches!(
        raw_cfa_frame_info(path, &cancelled),
        Err(ImageError::Cancelled { path: error_path }) if error_path == path
    ));
}

#[test]
fn load_raw_rejects_invalid_files() {
    #[derive(Debug)]
    struct InvalidRawCase {
        name: &'static str,
        contents: &'static [u8],
    }

    let directory = TempDir::new("invalid_raw_files");
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
        let path = directory.join(format!("{}.raf", case.name));
        fs::write(&path, case.contents).unwrap();
        assert!(
            load_raw(&path, &LoadContext::default()).is_err(),
            "{case:?}"
        );

        // The rejection happens while libraw is being opened, so the handle exists and has to
        // free itself on the way out — the failure path most likely to leak the instance. The file
        // reads back, so what libraw refuses is its contents: a `Raw` error naming the open.
        let error = load_raw_cfa(&path, &LoadContext::default()).unwrap_err();
        assert!(
            matches!(
                &error,
                ImageError::Raw {
                    source: RawError::Open(_),
                    ..
                }
            ),
            "{case:?}: contents libraw refuses should read as a Raw open error, got: {error}",
        );
    }
}

/// A synthetic camera file through LibRaw's `open_bayer`, unpacked: `samples` laid out `side`
/// square under one-pixel margins, RGGB, as `dump` describes it.
fn bayer_dump(samples: &[u16], side: usize, dump: BayerDump) -> Result<UnpackedRaw, RawError> {
    let libraw = Libraw::open_bayer(samples, side as u16, dump, &CancelToken::never())?;
    UnpackedRaw::unpack(libraw, Path::new("bayer-dump"), 1)
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
    let samples = |margin: u16| -> Vec<u16> {
        (0..SIDE * SIDE)
            .map(|index| {
                let (x, y) = (index % SIDE, index / SIDE);
                if x == 0 || y == 0 || x == SIDE - 1 || y == SIDE - 1 {
                    margin
                } else {
                    visible(x - 1, y - 1)
                }
            })
            .collect()
    };
    let context = LoadContext::default();
    let preview = |margin| {
        bayer_dump(&samples(margin), SIDE, black(1004))
            .unwrap()
            .into_linear_image(&context)
    };
    let dark = preview(0).unwrap();
    let bright = preview(65_535).unwrap();
    let science = bayer_dump(&samples(0), SIDE, black(1004))
        .unwrap()
        .into_cfa_image()
        .unwrap()
        .demosaic(MarkesteijnPasses::One, &CancelToken::never())
        .unwrap();
    assert_eq!(
        dark.dimensions(),
        ImageDimensions::new((SIDE - 2, SIDE - 2), 3)
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
    assert!(dark.metadata.provenance.as_ref().unwrap().clipped);
}

#[cfg(feature = "real-data")]
#[test]
fn load_raw_valid_file() {
    // Check mean is reasonable (not all zeros or all ones)
    use crate::internals::synthetic::metrics::pixel_stats;

    use crate::internals::init_tracing;
    use crate::internals::real_data::raw_frames;

    let path = raw_frames("Lights").swap_remove(0);

    init_tracing();

    let result = load_raw(&path, &LoadContext::default());
    assert!(result.is_ok(), "Failed to load {path:?}: {result:?}");

    let image = result.unwrap();

    // Validate dimensions
    assert!(image.dimensions().width() > 0);
    assert!(image.dimensions().height() > 0);
    assert_eq!(image.dimensions().channels(), 3); // RGB output

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

    let mean = (0..image.channels())
        .map(|channel| pixel_stats(image.channel(channel)).mean)
        .sum::<f64>()
        / image.channels() as f64;
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
        let mut raw = open_raw(&path, &LoadContext::default()).unwrap();
        let visible = raw.layout.active;
        assert_ne!(
            visible.width, visible.height,
            "a square frame cannot show a turn"
        );
        raw.libraw.set_saved_flip(6);
        let processed = raw.processed_by_libraw().unwrap();
        assert_eq!(
            (processed.width(), processed.height()),
            (visible.width, visible.height),
            "{}",
            path.display()
        );
    }
}

#[test]
fn camera_white_balance_is_canonicalized() {
    let bayer = Some(CfaType::Bayer(CfaPattern::Rggb));
    assert_eq!(
        camera_white_balance(bayer, [4.0, 2.0, 3.0, 2.0], 0),
        Some([2.0, 1.0, 1.5, 1.0])
    );
    assert_eq!(
        camera_white_balance(bayer, [2.0, 1.0, 1.5, 0.0], 0),
        Some([2.0, 1.0, 1.5, 1.0])
    );
    assert_eq!(
        camera_white_balance(
            Some(CfaType::XTrans(XTRANS_PATTERN)),
            [2.0, 1.0, 1.5, 9.0],
            0
        ),
        Some([2.0, 1.0, 1.5, 1.0])
    );
    assert_eq!(
        camera_white_balance(Some(CfaType::Mono), [2.0, 1.0, 1.5, 1.0], 0),
        None
    );
    // Samples that already carry the as-shot balance have unity left to apply, whatever the
    // multipliers say; a monochrome sensor has none either way.
    assert_eq!(
        camera_white_balance(bayer, [4.0, 2.0, 3.0, 2.0], 1),
        Some([1.0; 4])
    );
    assert_eq!(
        camera_white_balance(Some(CfaType::Mono), [4.0, 2.0, 3.0, 2.0], 1),
        None
    );
    // A sensor LibRaw processes itself (a linear DNG) still reports its multipliers.
    assert_eq!(
        camera_white_balance(None, [4.0, 2.0, 3.0, 2.0], 0),
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
    let cfa_type = Some(CfaType::Bayer(CfaPattern::Rggb));

    for input in invalid {
        assert!(
            camera_white_balance(cfa_type, input, 0).is_none(),
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
    let flags_for = |procflags: u8| {
        let mut samples = vec![2000u16; SIDE * SIDE];
        samples[7 * SIDE + 5] = 0;
        samples[0] = 0;
        samples[10 * SIDE + 10] = 62_309;
        samples[12 * SIDE + 12] = 62_308;
        let raw = bayer_dump(
            &samples,
            SIDE,
            BayerDump {
                procflags,
                ..black(1000)
            },
        )
        .unwrap();
        raw.decode_flags(raw.libraw.raw_image().unwrap()).unwrap()
    };

    let flags = flags_for(2);
    assert_eq!(flags.size(), Size2us::new(SIDE - 2, SIDE - 2));
    assert_eq!(flags.count(QualityFlags::NO_DATA), 1);
    assert_eq!(flags.at_pos(Vec2us::new(4, 6)), QualityFlags::NO_DATA);
    assert_eq!(flags.count(QualityFlags::SATURATED), 1);
    assert_eq!(flags.at_pos(Vec2us::new(9, 9)), QualityFlags::SATURATED);
    assert_eq!(flags.at_pos(Vec2us::new(11, 11)), QualityFlags::default());
    // A camera without the convention reads its zero as a value: not missing, not saturated.
    let flags = flags_for(0);
    assert_eq!(flags.count(QualityFlags::NO_DATA), 0);
    assert_eq!(flags.at_pos(Vec2us::new(4, 6)), QualityFlags::default());
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
    let samples: Vec<u16> = (0..SIDE * SIDE)
        .map(|index| {
            let (row, col) = (index / SIDE, index % SIDE);
            if col < 2 {
                let p = parity(row, col);
                (2048 + p + usize::from(row / 2 <= p)) as u16
            } else {
                3000
            }
        })
        .collect();
    let raw = bayer_dump(
        &samples,
        SIDE,
        BayerDump {
            mask: Some([0, 0, SIDE as i32, 2]),
            ..BayerDump::default()
        },
    )
    .unwrap();
    let color = &raw.libraw.data().color;
    assert_eq!(
        color.black, 2048,
        "unpack moved the least truncated mean into black"
    );
    for (row, col) in [(0, 0), (0, 1), (1, 0), (1, 1)] {
        let p = parity(row, col);
        let channel = libraw_filter_color(raw.visible_filters, row, col);
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
    let mut samples = vec![2000u16; SIDE * SIDE];
    samples[10 * SIDE + 10] = 20_000;
    let narrow = BayerDump {
        otherflags: 0x20,
        ..BayerDump::default()
    };
    assert!(matches!(
        bayer_dump(&samples, SIDE, narrow),
        Err(RawError::CorruptData)
    ));
    assert!(bayer_dump(&samples, SIDE, BayerDump::default()).is_ok());
    samples[10 * SIDE + 10] = 16_000;
    assert!(bayer_dump(&samples, SIDE, narrow).is_ok());
}

/// A cancel the load's token sees stops LibRaw at its next stage, and reads as the load's own
/// cancel rather than as a failure of the file.
#[test]
fn a_cancelled_token_stops_libraw() {
    let samples = vec![2000u16; 24 * 24];
    let cancel = CancelToken::new();
    let libraw = Libraw::open_bayer(&samples, 24, BayerDump::default(), &cancel).unwrap();
    cancel.cancel();
    let path = Path::new("bayer-dump");
    let error = UnpackedRaw::unpack(libraw, path, 1).unwrap_err();
    assert!(
        matches!(error, RawError::Unpack(LibrawCode::Cancelled)),
        "{error:?}"
    );
    assert!(matches!(
        ImageError::raw(path, error),
        ImageError::Cancelled { .. }
    ));
}

/// The peek counts what LibRaw holds beside the frame: the 24 × 24 × 2 = 1152-byte file it parses
/// in place, and the raw buffer of as many samples it unpacks into.
#[test]
fn the_peek_counts_the_file_and_the_raw_buffer() {
    let samples = vec![2000u16; 24 * 24];
    let libraw =
        Libraw::open_bayer(&samples, 24, BayerDump::default(), &CancelToken::never()).unwrap();
    let info = frame_info(&libraw).unwrap();
    assert_eq!(info.dimensions, ImageDimensions::new((22, 22), 1));
    assert_eq!(info.decoder_bytes, 1152 + 1152);
}

/// LibRaw's own processing, the path of a sensor lumos does not demosaic, run on a uniform field:
/// 2000 over a black of 1000, a span of 65535 − 1000 = 64 535, comes out as three equal planes of
/// LibRaw's 16-bit 1000 · 65535 / 64 535 over 65535 — within one 16-bit step of 1000 / 64 535 —
/// and declares that span.
#[test]
fn libraws_processing_declares_its_own_span() {
    const SIDE: usize = 24;
    let samples = vec![2000u16; SIDE * SIDE];
    let image = bayer_dump(&samples, SIDE, black(1000))
        .unwrap()
        .processed_by_libraw()
        .unwrap();
    assert_eq!(image.dimensions(), ImageDimensions::new((22, 22), 3));
    assert_eq!(image.metadata.domain.as_ref().unwrap().scale, 64_535.0);
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

/// The camera is LibRaw's normalized make and model, each read to its terminator and trimmed,
/// joined by a space; either alone when the other is empty, and `None` when both are.
#[test]
fn the_instrument_joins_the_normalized_make_and_model() {
    let field = |text: &str| {
        let mut field: [ffi::c_char; 64] = [0; 64];
        for (slot, byte) in field.iter_mut().zip(text.bytes()) {
            *slot = ffi::c_char::from_ne_bytes([byte]);
        }
        field
    };
    for (make, model, expected) in [
        ("Canon", "EOS 6D", Some("Canon EOS 6D")),
        ("Canon ", " EOS 6D", Some("Canon EOS 6D")),
        ("ZWO", "", Some("ZWO")),
        ("", "ASI2600MC", Some("ASI2600MC")),
        ("", "", None),
    ] {
        assert_eq!(
            instrument(&field(make), &field(model)).as_deref(),
            expected,
            "{make:?} {model:?}"
        );
    }
}
