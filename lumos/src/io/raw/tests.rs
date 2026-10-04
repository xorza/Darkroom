use common::TempDir;

use common::CancelToken;

use crate::internals::cfa::XTRANS_PATTERN;
use crate::io::raw::demosaic::bayer::CfaPattern;
use crate::io::raw::*;

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

#[cfg(unix)]
#[test]
fn load_raw_rejects_interior_nul_path() {
    use std::ffi::OsStr;
    use std::os::unix::ffi::OsStrExt;

    let path = Path::new(OsStr::from_bytes(b"invalid\0path.raf"));
    let error = load_raw(path, &LoadContext::default()).unwrap_err();
    assert!(error.to_string().contains("interior NUL byte"));
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

        // The rejection happens while libraw is being opened, so the state exists and has to free
        // itself on the way out — the failure path most likely to leak the instance.
        //
        // The file reads back, so what libraw refuses is its contents: a `Raw` error on every
        // platform, worded alike whether libraw was handed the path or the bytes.
        let error = LibrawState::open(&path).unwrap_err();
        assert!(
            matches!(&error, ImageError::Raw { .. }),
            "{case:?}: contents libraw refuses should read as a Raw error, got: {error}",
        );
        assert!(
            error.to_string().contains("Failed to open file"),
            "{case:?}: {error}",
        );
    }
}

/// A synthetic camera file through LibRaw's `open_bayer`: `samples` laid out `side` square under
/// one-pixel masked margins, RGGB, black `black`, maximum 65535; `procflags` 2 marks zeros dead.
fn bayer_dump(samples: &[u16], side: usize, procflags: u8, black: u32) -> UnpackedRaw {
    let mut bytes: Vec<u8> = samples
        .iter()
        .flat_map(|sample| sample.to_le_bytes())
        .collect();
    let data = bytes.as_mut_ptr();
    let len = bytes.len() as u32;
    // SAFETY: libraw_init returns a valid pointer or null.
    let inner = unsafe { sys::libraw_init(0) };
    assert!(!inner.is_null());
    // The state owns the bytes LibRaw reads in place, as it does a file read into memory: moving
    // the vector leaves its buffer where `data` points.
    let state = LibrawState {
        inner,
        buf: Some(bytes),
    };
    // SAFETY: the handle is valid, and the buffer lives as long as the state.
    let opened = unsafe {
        sys::libraw_open_bayer(
            state.as_ptr(),
            data,
            len,
            side as u16,
            side as u16,
            1,
            1,
            1,
            1,
            procflags,
            0x94, // RGGB in LibRaw's filter byte
            0,
            0,
            black,
        )
    };
    assert_eq!(opened, 0);
    unpack(state, Path::new("bayer-dump")).unwrap()
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
    let preview = |margin| bayer_dump(&samples(margin), SIDE, 0, 1004).into_linear_image(&context);
    let dark = preview(0).unwrap();
    let bright = preview(65_535).unwrap();
    let science = bayer_dump(&samples(0), SIDE, 0, 1004)
        .into_cfa_image()
        .unwrap()
        .demosaic(&CancelToken::never())
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

/// The header dimensions are the sensor's, as the file declares them: the visible area, one
/// sample per photosite, whatever the demosaic made of it.
#[cfg(feature = "real-data")]
#[test]
fn load_raw_dimensions_match() {
    use crate::internals::real_data::raw_frames;

    let path = raw_frames("Lights").swap_remove(0);
    let image = load_raw(&path, &LoadContext::default()).unwrap();
    assert_eq!(
        image.metadata.header_dimensions,
        [image.dimensions().height(), image.dimensions().width(), 1]
    );
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
        let raw = open_raw(&path).unwrap();
        let visible = raw.layout.active;
        assert_ne!(
            visible.width, visible.height,
            "a square frame cannot show a turn"
        );
        // SAFETY: the instance is open and unpacked.
        unsafe { (*raw.libraw.as_ptr()).rawdata.sizes.flip = 6 };
        let processed = raw.demosaic_libraw_fallback().unwrap();
        assert_eq!(
            (processed.dimensions.width(), processed.dimensions.height()),
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
        canonical_camera_white_balance(bayer, [4.0, 2.0, 3.0, 2.0]),
        Some([2.0, 1.0, 1.5, 1.0])
    );
    assert_eq!(
        canonical_camera_white_balance(bayer, [2.0, 1.0, 1.5, 0.0]),
        Some([2.0, 1.0, 1.5, 1.0])
    );
    assert_eq!(
        canonical_camera_white_balance(Some(CfaType::XTrans(XTRANS_PATTERN)), [2.0, 1.0, 1.5, 9.0]),
        Some([2.0, 1.0, 1.5, 1.0])
    );
    assert_eq!(
        canonical_camera_white_balance(Some(CfaType::Mono), [2.0, 1.0, 1.5, 1.0]),
        None
    );
    // A sensor LibRaw processes itself (a linear DNG) still reports its multipliers.
    assert_eq!(
        canonical_camera_white_balance(None, [4.0, 2.0, 3.0, 2.0]),
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
            canonical_camera_white_balance(cfa_type, input).is_none(),
            "{input:?}"
        );
    }
}

/// The raw values settle two flags at decode (review items 8.3 and 9.3). LibRaw's `open_bayer`
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
        bayer_dump(&samples, SIDE, procflags, 1000)
            .decode_flags()
            .unwrap()
            .unwrap()
    };

    let flags = flags_for(2);
    assert_eq!(flags.size(), Size2us::new(SIDE - 2, SIDE - 2));
    assert_eq!(flags.count(Flags::NO_DATA), 1);
    assert_eq!(flags.at_pos(Vec2us::new(4, 6)), Flags::NO_DATA);
    assert_eq!(flags.count(Flags::SATURATED), 1);
    assert_eq!(flags.at_pos(Vec2us::new(9, 9)), Flags::SATURATED);
    assert_eq!(flags.at_pos(Vec2us::new(11, 11)), Flags::default());
    // A camera without the convention reads its zero as a value: not missing, not saturated.
    let flags = flags_for(0);
    assert_eq!(flags.count(Flags::NO_DATA), 0);
    assert_eq!(flags.at_pos(Vec2us::new(4, 6)), Flags::default());
}
